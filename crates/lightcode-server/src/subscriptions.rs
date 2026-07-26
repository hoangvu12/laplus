//! Server-streaming subscriptions — the second framing mechanism on this wire.
//!
//! A subscription is not a distinct kind of call at the envelope level: it
//! arrives as an ordinary [`crate::wire::ClientMessage::Request`] and nothing
//! in the frame says it will stream. What differs is everything after that —
//! values come back as `Chunk`s under the same `requestId`, the client
//! acknowledges each one, and the call ends when the client cancels it.
//!
//! Three rules from `docs/socket-wire-format.md` shape this module, and each
//! one is a place where an obvious implementation would be wrong:
//!
//! - **`Ack` is real back-pressure.** The server sends at most one
//!   un-acknowledged chunk per request and stops until the client answers.
//!   Fixture 05 shows the reference server stalling a *committed* change
//!   behind a withheld acknowledgement for two seconds. A server that pushed
//!   freely would not fail visibly against the UI — the UI acknowledges
//!   everything — but a busy subscription's memory would go from bounded to
//!   unbounded.
//! - **`values` batches.** It is an array and a conforming client iterates it.
//!   Events that pile up behind an outstanding acknowledgement are sent as one
//!   chunk rather than a queue of frames.
//! - **A client-initiated unsubscribe ends as `Failure`/`Interrupt`, not
//!   `Success`.** That is the captured behaviour, and a client reads it as a
//!   normal end. The terminal frame is written by the connection rather than
//!   here — see [`Subscriptions::interrupt`].
//!
//! What is deliberately *not* here: any knowledge of what is being streamed.
//! A source is a snapshot function and a feed of updates ([`EventSource`]), so
//! the orchestration, terminal, file-tree and git subscriptions can all arrive
//! later without touching this file.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::wire::ServerMessage;

/// How many published events a subscriber may fall behind before its backlog
/// is thrown away and replaced with a fresh snapshot.
///
/// This is the bound that makes ignoring `Ack` merely wasteful rather than
/// fatal. A subscriber that stops acknowledging cannot make the server hold an
/// unbounded queue on its behalf: past this many events the cheapest correct
/// answer is to resend the world, and a snapshot supersedes everything it
/// missed anyway.
///
/// Also the ceiling on how many values one chunk carries. The two are the same
/// number on purpose — a subscriber that drains a full backlog in one chunk
/// can never be the reason the next one lags.
pub const BACKLOG: usize = 64;

/// The server side of one subscription: how to describe the world from
/// scratch, and how to hear about changes to it.
///
/// The snapshot is a function rather than a value because it is needed twice —
/// once when the subscription opens, and again whenever a subscriber falls far
/// enough behind that resynchronising beats catching up.
pub struct EventSource {
    description: Box<dyn Fn() -> Vec<Value> + Send>,
    updates: broadcast::Receiver<Value>,
}

impl EventSource {
    pub fn new(
        description: impl Fn() -> Vec<Value> + Send + 'static,
        updates: broadcast::Receiver<Value>,
    ) -> EventSource {
        EventSource {
            description: Box::new(description),
            updates,
        }
    }

    /// What this source would open with, right now.
    ///
    /// The primitive the rest of the module is built from: [`Self::resynchronise`]
    /// is this plus a drain. A source may answer with *no* events — a
    /// description it cannot currently produce is better withheld than replaced
    /// with an empty one, which would be a positive claim about the world
    /// rather than an absence of one.
    pub fn describe(&self) -> Vec<Value> {
        (self.description)()
    }

    /// Describe the world again, **discarding the backlog it supersedes**.
    ///
    /// Discarding is the whole point and it is easy to get wrong: a lagged
    /// `broadcast` receiver is not emptied, it is fast-forwarded to the oldest
    /// value it still holds. Without the drain, a resynchronisation would send
    /// a snapshot and then deliver the very events it was sent *instead of* —
    /// and since the client applies each one as a wholesale replacement, its
    /// configuration would walk backwards through values the server had
    /// already left before arriving where the snapshot had put it.
    ///
    /// Drained *before* the snapshot is taken, not after. An event landing
    /// between the two is then merely delivered twice, which a projection of
    /// wholesale replacements absorbs; the other order would drop it.
    fn resynchronise(&mut self) -> Vec<Value> {
        // `Lagged` keeps the drain going: the sender can outrun it, and the
        // loop is done only when the receiver is empty or the feed has ended.
        while let Ok(_) | Err(TryRecvError::Lagged(_)) = self.updates.try_recv() {}
        self.describe()
    }
}

impl fmt::Debug for EventSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EventSource")
    }
}

/// The subscriptions open on one connection, keyed by the `requestId` that
/// opened them.
///
/// Per-connection rather than global because that is the lifetime that
/// matters: when a socket goes — cleanly, abruptly, or because the server is
/// stopping — every stream on it must go with it. Owning the registry inside
/// the connection makes that the default rather than something to remember.
#[derive(Debug)]
pub struct Subscriptions {
    open: HashMap<String, Open>,
    live: Arc<AtomicUsize>,
    frames: mpsc::Sender<String>,
}

#[derive(Debug)]
struct Open {
    pump: JoinHandle<()>,
    acks: mpsc::Sender<()>,
}

impl Open {
    /// Stop the pump and wait for it to actually be gone.
    ///
    /// The wait is what makes the terminal `Exit` safe to send afterwards: an
    /// `abort` alone only asks, and a pump cancelled mid-write could otherwise
    /// land a chunk after the frame that ended the stream.
    async fn stop(self) {
        self.pump.abort();
        let _ = self.pump.await;
    }
}

impl Subscriptions {
    pub fn new(live: Arc<AtomicUsize>, frames: mpsc::Sender<String>) -> Subscriptions {
        Subscriptions {
            open: HashMap::new(),
            live,
            frames,
        }
    }

    /// Open a subscription under `request_id` and start streaming it.
    pub async fn start(&mut self, request_id: String, source: EventSource) {
        let (acks, acknowledged) = mpsc::channel(1);
        let live = LiveSubscription::open(&self.live);
        let pump = tokio::spawn(pump(
            request_id.clone(),
            source,
            self.frames.clone(),
            acknowledged,
            live,
        ));

        if let Some(replaced) = self.open.insert(request_id, Open { pump, acks }) {
            // Request ids were strictly monotonic in every capture and never
            // reused, so this is unobserved territory — open question 8 in
            // `docs/socket-wire-format.md`. Replacing rather than refusing
            // keeps the registry honest either way: the client has already
            // rebound the id to the new call, so the old stream has nowhere
            // left to deliver.
            eprintln!("lightcode: a second subscription reused an open request id");
            replaced.stop().await;
        }
    }

    /// Release the pump to send its next chunk.
    ///
    /// An acknowledgement for something that is not streaming is ignored, not
    /// an error: it can legitimately race the end of a stream, and the client
    /// has no way to know it lost.
    pub fn acknowledge(&self, request_id: &str) {
        if let Some(open) = self.open.get(request_id) {
            // One slot, because the pump only ever waits for one. A duplicate
            // acknowledgement has nowhere to go and is dropped rather than
            // buying the client an extra un-acknowledged chunk.
            let _ = open.acks.try_send(());
        }
    }

    /// End a subscription. Returns whether there was one, which is how the
    /// caller knows to write the terminal `Exit` — a cancelled unary call, or
    /// a cancellation that lost a race with the stream's own end, must not
    /// produce a second answer for the same id.
    pub async fn interrupt(&mut self, request_id: &str) -> bool {
        match self.open.remove(request_id) {
            Some(open) => {
                open.stop().await;
                true
            }
            None => false,
        }
    }

    /// End every subscription and wait for them all. What a connection calls
    /// on its way out, whichever way it is going.
    pub async fn shutdown(mut self) {
        for (_, open) in self.open.drain() {
            open.stop().await;
        }
    }

    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

impl Drop for Subscriptions {
    /// A backstop for the paths that cannot await — a panic unwinding through
    /// the connection loop. [`Subscriptions::shutdown`] is the ordinary way
    /// out and waits properly; this one only guarantees the pumps stop.
    fn drop(&mut self) {
        for open in self.open.values() {
            open.pump.abort();
        }
    }
}

/// Stream one subscription until it is cancelled or its connection goes.
///
/// The loop is "send what is pending, wait to be acknowledged, gather what
/// arrived meanwhile" — which is where both the back-pressure and the batching
/// come from. Neither is an extra feature layered on top; they are the same
/// property seen from two sides.
async fn pump(
    request_id: String,
    mut source: EventSource,
    frames: mpsc::Sender<String>,
    mut acknowledged: mpsc::Receiver<()>,
    _live: LiveSubscription,
) {
    let mut pending = source.resynchronise();

    loop {
        if !pending.is_empty() {
            let chunk = ServerMessage::Chunk {
                request_id: request_id.clone(),
                values: std::mem::take(&mut pending),
            };
            // Both of these fail only when the connection has gone: the frame
            // queue closes with the writer, and the acknowledgement channel
            // with the registry entry.
            if frames.send(chunk.to_frame()).await.is_err() {
                return;
            }
            if acknowledged.recv().await.is_none() {
                return;
            }
        }

        match source.updates.recv().await {
            Ok(event) => {
                pending.push(event);
                // Whatever else is already waiting rides in the same chunk.
                while pending.len() < BACKLOG {
                    match source.updates.try_recv() {
                        Ok(event) => pending.push(event),
                        // Fell behind mid-gather. The snapshot supersedes what
                        // was already collected too, so it replaces `pending`
                        // rather than joining it.
                        Err(TryRecvError::Lagged(_)) => {
                            pending = source.resynchronise();
                            break;
                        }
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
            }
            // Too far behind to catch up. A snapshot supersedes every event
            // that was dropped, so this is a resynchronisation rather than a
            // gap — the client's projection treats one as a reset.
            Err(RecvError::Lagged(_)) => pending = source.resynchronise(),
            // Nothing will publish again. Every source outlives its
            // subscribers today, so this is unreachable in practice.
            Err(RecvError::Closed) => return,
        }
    }
}

/// Keeps the live-subscription gauge honest however a pump ends — cancelled,
/// disconnected, or with the task dropped out from under it.
#[derive(Debug)]
struct LiveSubscription {
    live: Arc<AtomicUsize>,
}

impl LiveSubscription {
    fn open(live: &Arc<AtomicUsize>) -> LiveSubscription {
        live.fetch_add(1, Ordering::Relaxed);
        LiveSubscription {
            live: Arc::clone(live),
        }
    }
}

impl Drop for LiveSubscription {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A source whose snapshot is a counter, so a resynchronisation is
    /// distinguishable from the snapshot sent at subscribe time.
    fn counting_source(updates: &broadcast::Sender<Value>) -> (EventSource, Arc<AtomicUsize>) {
        let resyncs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&resyncs);
        let source = EventSource::new(
            move || vec![json!({"snapshot": counter.fetch_add(1, Ordering::Relaxed)})],
            updates.subscribe(),
        );
        (source, resyncs)
    }

    fn chunk_values(frame: &str) -> Vec<Value> {
        let frame: Value = serde_json::from_str(frame).expect("valid json");
        assert_eq!(frame["_tag"], "Chunk");
        frame["values"].as_array().expect("an array").clone()
    }

    #[tokio::test]
    async fn a_pump_opens_with_a_snapshot_and_then_waits_to_be_acknowledged() {
        let (updates, _) = broadcast::channel(BACKLOG);
        let (frames, mut written) = mpsc::channel(BACKLOG);
        let live = Arc::new(AtomicUsize::new(0));
        let mut subscriptions = Subscriptions::new(Arc::clone(&live), frames);

        let (source, _) = counting_source(&updates);
        subscriptions.start("0".to_string(), source).await;
        assert_eq!(live.load(Ordering::Relaxed), 1);

        let snapshot = written.recv().await.expect("a snapshot chunk");
        assert_eq!(chunk_values(&snapshot), vec![json!({"snapshot": 0})]);

        // Un-acknowledged: the update is held rather than sent.
        let _ = updates.send(json!({"update": 1}));
        assert!(written.try_recv().is_err());

        subscriptions.acknowledge("0");
        let update = written.recv().await.expect("the held update");
        assert_eq!(chunk_values(&update), vec![json!({"update": 1})]);

        subscriptions.shutdown().await;
        assert_eq!(live.load(Ordering::Relaxed), 0);
    }

    /// Everything published behind one outstanding acknowledgement arrives as
    /// a single chunk, in the order it was published.
    #[tokio::test]
    async fn events_held_behind_an_acknowledgement_are_batched_in_order() {
        let (updates, _) = broadcast::channel(BACKLOG);
        let (frames, mut written) = mpsc::channel(BACKLOG);
        let mut subscriptions = Subscriptions::new(Arc::new(AtomicUsize::new(0)), frames);

        let (source, _) = counting_source(&updates);
        subscriptions.start("0".to_string(), source).await;
        written.recv().await.expect("a snapshot chunk");

        for index in 0..3 {
            let _ = updates.send(json!({"update": index}));
        }
        subscriptions.acknowledge("0");

        let batch = chunk_values(&written.recv().await.expect("a batch"));
        assert_eq!(
            batch,
            vec![
                json!({"update": 0}),
                json!({"update": 1}),
                json!({"update": 2})
            ]
        );

        subscriptions.shutdown().await;
    }

    /// Past the backlog the pump stops trying to catch up and describes the
    /// world again instead. The bound is what keeps a client that never
    /// acknowledges from costing unbounded memory.
    #[tokio::test]
    async fn a_pump_that_falls_past_the_backlog_resynchronises() {
        let (updates, _) = broadcast::channel(BACKLOG);
        let (frames, mut written) = mpsc::channel(BACKLOG);
        let mut subscriptions = Subscriptions::new(Arc::new(AtomicUsize::new(0)), frames);

        let (source, resyncs) = counting_source(&updates);
        subscriptions.start("0".to_string(), source).await;
        written.recv().await.expect("a snapshot chunk");
        assert_eq!(resyncs.load(Ordering::Relaxed), 1);

        for index in 0..(BACKLOG * 2 + 1) {
            let _ = updates.send(json!({"update": index}));
        }
        subscriptions.acknowledge("0");

        let resync = chunk_values(&written.recv().await.expect("a resynchronisation"));
        assert_eq!(resync, vec![json!({"snapshot": 1})]);
        assert_eq!(resyncs.load(Ordering::Relaxed), 2);

        // The superseded backlog is gone, not merely overtaken. A lagged
        // receiver is fast-forwarded to the oldest value it still holds, so a
        // resync that only re-describes the world would then deliver the very
        // events it was sent instead of.
        subscriptions.acknowledge("0");
        let _ = updates.send(json!({"update": "after"}));
        let next = chunk_values(&written.recv().await.expect("the next update"));
        assert_eq!(
            next,
            vec![json!({"update": "after"})],
            "a stale backlog followed the snapshot"
        );

        subscriptions.shutdown().await;
    }

    /// Ending one subscription leaves its neighbours streaming. The registry
    /// is keyed by request id, and this is the test that says so.
    #[tokio::test]
    async fn subscriptions_end_one_at_a_time() {
        let (updates, _) = broadcast::channel(BACKLOG);
        let (frames, mut written) = mpsc::channel(BACKLOG);
        let live = Arc::new(AtomicUsize::new(0));
        let mut subscriptions = Subscriptions::new(Arc::clone(&live), frames);

        for id in ["0", "1"] {
            let (source, _) = counting_source(&updates);
            subscriptions.start(id.to_string(), source).await;
            written.recv().await.expect("a snapshot chunk");
        }
        assert_eq!(live.load(Ordering::Relaxed), 2);

        assert!(subscriptions.interrupt("0").await);
        assert_eq!(live.load(Ordering::Relaxed), 1);
        assert_eq!(subscriptions.len(), 1);

        // The survivor still streams, and the departed one no longer does.
        subscriptions.acknowledge("1");
        let _ = updates.send(json!({"update": 0}));
        let held = written.recv().await.expect("the survivor's update");
        assert_eq!(chunk_values(&held), vec![json!({"update": 0})]);
        assert!(written.try_recv().is_err(), "only one subscription answered");

        subscriptions.shutdown().await;
        assert_eq!(live.load(Ordering::Relaxed), 0);
    }

    /// Neither an acknowledgement nor a cancellation for something that is not
    /// streaming may be an error — both are ordinary client traffic.
    #[tokio::test]
    async fn acknowledging_or_interrupting_an_unknown_request_does_nothing() {
        let (frames, _written) = mpsc::channel(BACKLOG);
        let mut subscriptions = Subscriptions::new(Arc::new(AtomicUsize::new(0)), frames);

        subscriptions.acknowledge("41");
        assert!(!subscriptions.interrupt("41").await);
        assert!(subscriptions.is_empty());
    }
}
