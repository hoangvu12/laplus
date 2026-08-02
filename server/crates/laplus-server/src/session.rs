//! Running a session: the loop that owns an agent's lifetime, and the trait it
//! runs one over.
//!
//! Nothing here knows which agent is behind the session. What varies between one
//! agent and the next is the I/O — starting a process, reading what it says,
//! writing a prompt, a decision, a stop — and that is behind [`Driver`].
//! Everything this file does *around* those verbs is written once: baselines,
//! checkpoints, session epochs, settling, and every session event the client
//! reads. A second driver reuses this rather than copying it, because
//! checkpoints, epochs and settling drifting apart between two agents is a class
//! of bug nothing would catch.
//!
//! [`crate::turn`] is the one implementation there is, and it drives the
//! `claude` CLI.
//!
//! ## Where the line between the two is drawn
//!
//! ADR-0001 drew it and this file follows it rather than re-deciding it: **the
//! decoder is mirrored and shared, the encoder belongs to the driver.** So
//! [`crate::settling`] — how a session status settles a turn — is shared, and
//! what a turn's ending *encodes to* is the driver's, because that knows which
//! agent reported it.
//!
//! [`Decided`] is what a driver answers with, and it is the shared vocabulary in
//! both directions: the driver reads its own protocol and says what the
//! conversation is owed, in [`Change`]s the client's own contract describes.
//! What it must *not* answer with is anything shaped like the agent it ran —
//! that is the whole of what stops this loop knowing.
//!
//! [`crate::approval::ApprovalRequest`] is the provider-neutral request this loop
//! holds while an agent waits. Each protocol decodes into it and each driver owns
//! its answer encoding; provider correlation data crosses the loop opaquely.
//! [`Drift`] and [`TokenUsage`] still live in Claude's protocol module even though
//! they describe a session, which remains a placement loose end.
//!
//! ## One long-lived driver, not one per turn
//!
//! A session is a task that owns an agent and a channel of prompts. Dispatching
//! a turn puts a prompt in the channel and returns; it never waits for a process
//! to exist, which is what lets the socket acknowledge the developer's message
//! immediately. The task starts the agent on its first prompt and then stays,
//! because the agent stays: re-spawning per turn would throw away the
//! conversation the developer is having.
//!
//! Everything the task does after that is one loop over three sources — an event
//! from the agent, another prompt, or a signal from the developer — and it ends
//! when the agent's output does or when the developer stops the session. A prompt
//! channel that closes is the gentler "no more turns": the agent is told, and the
//! loop keeps draining what it still owes. Every ending reaps the child.
//!
//! ## A permission request is the one thing the agent waits for
//!
//! Everything else a driver reports is the agent talking. An approval request is
//! the agent *asking*, and it has stopped until it is answered — so the loop
//! polls the decision channel unconditionally, where it deliberately does not
//! poll for a second prompt while one is in flight. A decision deferred behind a
//! queued turn would be the deadlock ticket 13 was about.
//!
//! Three things follow from the agent being stopped rather than merely busy:
//!
//! - **The request is remembered here, not in the fold.** The client answers by
//!   naming an id; what has to go back to the agent is the whole request,
//!   because an approval carries the input the tool will run with. [`Driving`]
//!   is where the two are joined.
//! - **The agent is written to before the resolution is published.** The panel
//!   closing is what the developer sees; the write is what unsticks the
//!   conversation. Publishing first would risk a closed panel over a session
//!   still stopped.
//! - **Whatever is still outstanding when the driver ends is closed.** The
//!   client's panel is folded out of `approval.requested` minus
//!   `approval.resolved` and those activities are *stored*, so a request left
//!   open would be a composer the developer cannot type into — after a restart as
//!   well as before one. Ending the session settles them as cancelled, which is
//!   what actually happened.
//!
//! ## Stopping a turn is not stopping a session, and the difference is the point
//!
//! A turn is stopped by [`Driver::interrupt`], which leaves the child running —
//! that is what makes the correction the developer types next a *correction*
//! rather than the first message of a conversation that has forgotten what it
//! was about. Two things follow, and neither is the driver's:
//!
//! - **The turn settles twice, on purpose, and the first one is the click.**
//!   `thread.turn-interrupt-requested` goes up the moment the request has been
//!   written, and the client's reducer moves the latest turn to `interrupted` on
//!   it — so the turn stops being reported as running when the developer stops
//!   it rather than when the agent gets round to admitting it. The session
//!   follows when the driver reports the turn ended. Both are the contract's own
//!   vocabulary; neither alone is enough, because the event does not describe the
//!   session and the session status arrives too late to be what the developer
//!   sees.
//! - **The session change at the end of a turn is conditional.** A developer who
//!   stops the agent can send the next turn while this one is still winding
//!   down, and by then the session describes *that* turn. Settling it here would
//!   report a turn that had just started as finished.
//!
//! An interrupt for a turn that is not running is a no-op rather than an error,
//! at every layer: the client sends one when it believes nothing is running, the
//! registry takes it with no session to route it to, and this loop drops it with
//! no turn to stop. See [`interrupt`], where the three races are named.
//!
//! The other act — ending the session — arrives on the same channel as
//! [`Signal::Stop`] and is answered by *leaving this loop*. Everything below the
//! loop is the ending a session is owed, and [`Driver::stop`] at the end of it
//! reaps the child under a bound of its own. That bound is why it is a signal
//! rather than only the prompt channel closing, which is the gentler "no more
//! turns" a shutdown and a deleted project use: the case the stop command exists
//! for is an agent that is not answering, and closing a pipe at one of those is a
//! hope. Two things follow from the developer having asked, and both are marked
//! in the ending: the turn it cut short is not reported as the agent having died,
//! and it gets no checkpoint — because no checkpoint status means "the developer
//! ended the session" and both the ones there are would relabel the turn.
//!
//! ## A turn is also a point in time, and this is the only place that knows when
//!
//! Ticket 20 asks for a turn to be reviewable as a diff, and a diff needs a
//! *before*. Nothing in git records what the working tree looked like when the
//! developer pressed enter, so this file records it: a checkpoint is written
//! before the prompt reaches the agent and again when the turn ends, and
//! [`crate::checkpoints`] owns everything about how. Two properties come from
//! the placement rather than from that module:
//!
//! - **The baseline is awaited before the agent is written to.** A capture
//!   racing the agent's first edit would record a tree that already had it, and
//!   the turn would show a diff missing its own first change.
//! - **The ending is recorded before the next prompt is taken.** That is what
//!   chains the checkpoints: turn two's baseline is turn one's ending, so a
//!   conversation is a sequence of adjoining ranges with no gap for a hand edit
//!   to fall into unattributed.
//!
//! Both are `await`s on a blocking thread in a loop that is otherwise reading a
//! child's output, and both cost a `git add -A` on the project. That is the
//! price of the feature and it is paid twice per conversation plus once per
//! turn.
//!
//! ## The mode and model a turn is answered under
//!
//! Whatever a driver's agent reads its latitude from, one child serves a whole
//! conversation — so a developer who changes the mode or the model mid-way is
//! answered by a process still running under the old one until it is told.
//! [`retune`] is what tells it, and three properties fall out of where it is
//! called rather than being enforced anywhere:
//!
//! - **The moment is the turn's own dispatch**, not the moment the picker's
//!   command commits. What a turn wants rides on its [`Prompt`], so it is spent
//!   when that turn is handed to the agent and not before: a change made while a
//!   turn is running does not move the rules under it, and a turn queued behind
//!   another keeps its own. Ticket 02 of `thread-lifecycle` and the spec's story
//!   29.
//! - **The comparison is against [`Start`]**, which is this session's account of
//!   what the child in front of it is running under rather than of what the
//!   thread last said. That is what makes "no push when nothing changed" true,
//!   and it is why a successful push has to move `Start` — every session event
//!   this file publishes reads it.
//! - **A refusal arrives later**, like everything else the agent says, so
//!   putting `Start` back is a thing a driver asks for on a [`Decided`] and this
//!   loop spends.

use std::collections::HashMap;
use std::future::Future;

use serde_json::{json, Value};

use crate::approval::ApprovalRequest;
use crate::clock::now_iso;
use crate::config::{ClaudeSettings, Settings};
use crate::protocol::{Drift, TokenUsage};
use crate::settling::SessionStatus;
use crate::threads::{
    Activity, Answered, Change, Prompt, Retune, Session, Signal, Thread, Threads,
    UserInputAnswered,
};
use crate::worklog::{Call, Decision};

/// What runs one agent behind a session: it owns the process, speaks that
/// agent's protocol, and answers with the changes a conversation is owed.
///
/// **The I/O verbs and nothing else.** Everything a session does around them —
/// baselines, checkpoints, epochs, settling, publishing — is [`drive`]'s, and is
/// written once for every driver there will ever be. What is left here is the
/// nine things that can only be done by whoever owns the process: open it, take
/// what it said, give it a turn, stop that turn, answer what it asked, ask how
/// full its window is, move it onto another mode or model, say there will be no
/// more turns, and reap it.
///
/// Seven of the nine are the codex-driver spec's own list. The two it does not
/// name are here because they are I/O and nothing else could do them: asking how
/// full the window is, which only the driver that asked can address to the agent
/// that answers, and closing the agent's input, which is a session saying "no
/// more turns" without giving the child up.
///
/// Two of the nine are worth being exact about, because a second implementation
/// will get them wrong otherwise:
///
/// - **[`Driver::next`] must be cancel-safe.** It is one arm of a `select!`, so
///   it is dropped unfinished every time a prompt or a signal wins the race. An
///   implementation that awaits anything *after* it has taken an event off its
///   transport will lose that event the first time it is dropped there. Take,
///   then decide synchronously; the writes belong to the other verbs, which the
///   loop awaits on their own.
/// - **[`Driver::stop`] says what the ending is owed.** It is the last moment
///   anybody can ask the agent's own account of itself — the reason a resume was
///   refused, the last thing it said on the way out — and those are sentences
///   only the driver can write.
pub(crate) trait Driver: Send + Sized {
    /// Whether a prompt received during a running turn belongs to that same
    /// turn instead of waiting to begin another one.
    const STEERS_ACTIVE_TURN: bool = false;

    /// Start the agent for this session, or say why not in a sentence the
    /// developer will read in the conversation.
    fn open(start: &Start) -> impl Future<Output = Result<Opened<Self>, String>> + Send;

    /// The next thing the agent said, folded and translated into what the
    /// conversation is owed. `None` once it has stopped producing.
    ///
    /// [`Driving`] is lent rather than owned by the driver because it is what
    /// *both* sides need: the loop reads the turn in flight to decide what to do
    /// next, and the translation writes the turn's tool calls and the requests
    /// it is waiting on into it.
    ///
    /// Cancel-safe — see this trait's documentation, where the reason is not
    /// optional.
    fn next(&mut self, driving: &mut Driving) -> impl Future<Output = Option<Decided>> + Send;

    /// Give the agent one turn.
    fn send(&mut self, text: &str) -> impl Future<Output = std::io::Result<()>> + Send;

    /// Stop the turn in flight without ending the session.
    ///
    /// Nothing is waited for: what the request did arrives through
    /// [`Driver::next`] like everything else, and `request_id` is what matches
    /// the answer to it.
    fn interrupt(&mut self, request_id: &str) -> impl Future<Output = std::io::Result<()>> + Send;

    /// Answer something the agent has stopped for.
    ///
    /// The half of the outbound protocol with a deadline: the agent is not
    /// merely busy, it is waiting, so a write that failed here is a wedged
    /// conversation rather than a lost message.
    fn answer(
        &mut self,
        asked: &ApprovalRequest,
        reply: Reply<'_>,
    ) -> impl Future<Output = std::io::Result<()>> + Send;

    /// Ask how full the context window is.
    ///
    /// Asked only where the driver said it was worth asking ([`Driving`]'s
    /// `unmeasured`), so an agent that cannot answer the question is never asked
    /// it. Nothing waits for the answer and nothing breaks if it never comes.
    fn measure(&mut self, request_id: &str) -> impl Future<Output = std::io::Result<()>> + Send;

    /// Move the running agent onto the mode or model this turn asks for.
    ///
    /// Whether it landed is the agent's to say, and it says so through
    /// [`Driver::next`] under the same `request_id` — see [`retune`], which is
    /// what decides there is anything to push at all.
    fn retune(
        &mut self,
        request_id: &str,
        asked: &Pushed,
    ) -> impl Future<Output = std::io::Result<()>> + Send;

    /// Say there will be no more turns, without waiting for the agent to finish
    /// the one it is on.
    ///
    /// The half of [`Driver::stop`] a still-draining session wants on its own:
    /// the loop has output left to publish and cannot give the agent up yet.
    fn close_input(&mut self);

    /// Reap the agent, and say what this session's ending is owed.
    ///
    /// `asked_to_stop` is the one thing the driver cannot know: a turn cut short
    /// because the developer ended the session is not a turn the agent died in
    /// the middle of, and only the loop heard the click.
    fn stop(
        self,
        driving: &mut Driving,
        asked_to_stop: bool,
    ) -> impl Future<Output = Reaped> + Send;
}

/// What the developer said back to something the agent stopped for.
///
/// Two shapes because the client has two panels — an approval, and a question
/// the composer draws — and they are one verb because to the agent they are one
/// thing: the answer to the request it is waiting on. Which words go on the wire
/// is the driver's, which is why the contract's own vocabulary is what travels
/// here.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Reply<'a> {
    /// What the developer decided about an approval request.
    Decided(Decision),
    /// What the developer typed into a question. Carried unread — the answers
    /// are their own words, and nothing here has any business parsing them.
    Answers(&'a Value),
}

/// A driver and everything its successful startup already decided.
///
/// Startup sits outside the selectable event loop, so these changes cannot be
/// dropped when a prompt and an agent event are ready together. Claude has
/// nothing to report here; Codex returns its continuity handle and, after a
/// recoverable resume refusal, the activity explaining its fresh thread.
pub(crate) struct Opened<D> {
    pub(crate) driver: D,
    pub(crate) decided: Decided,
}

/// What a driver leaves behind when the agent has gone.
///
/// Two sentences and both are the driver's, because both quote the agent: a
/// resume it would not honour, and a death mid-turn. The loop decides what to do
/// with them — which is published, which becomes the session's `lastError` —
/// because that is a question about the conversation rather than about the
/// agent.
#[derive(Debug, Default)]
pub(crate) struct Reaped {
    /// The conversation can be read but not continued, and why.
    pub(crate) refused: Option<String>,
    /// The agent went away in the middle of a turn, and what it said on the way
    /// out.
    pub(crate) death: Option<String>,
}

/// Everything a session needs to start an agent, gathered while the thread is
/// known and carried into the task that will need it.
#[derive(Debug, Clone)]
pub struct Start {
    pub thread_id: String,
    /// The project's folder. The agent's working directory, which is what makes
    /// a relative path in the transcript mean what the developer thinks.
    pub workspace_root: String,
    /// The model the child is running under — the launch flag at first, and then
    /// whatever a successful push has moved it to. See [`retune`].
    pub model: Option<String>,
    /// The runtime mode the child is running under, on the same terms as
    /// [`Start::model`].
    ///
    /// **Every session event the driver publishes reads this**, which is why a
    /// push that succeeds has to move it and a push that is refused has to leave
    /// it alone: the badge beside the session state is a claim about what the
    /// agent is doing, not about what was asked of it.
    pub runtime_mode: String,
    /// Opaque continuation data for this exact provider instance.
    pub resume_cursor: Option<crate::provider::ResumeCursor>,
    pub provider: crate::provider::ProviderIdentity,
    /// Read once, when the turn is dispatched. A settings change mid-session
    /// does not move a running agent, which is honest — the process was started
    /// with the old value and cannot be told otherwise.
    /// Selected from the conversation's registry entry together, so a driver's
    /// implementation cannot be paired with another driver's settings.
    pub driver: DriverStart,
}

#[derive(Debug, Clone)]
pub enum DriverStart {
    Claude(ClaudeSettings),
    Codex(crate::config::CodexSettings),
    OpenCode(crate::config::OpenCodeSettings),
}

#[derive(Debug, Clone)]
pub struct PreparedDriver {
    identity: crate::provider::ProviderIdentity,
    driver: DriverStart,
}

impl DriverStart {
    pub(crate) fn claude(&self) -> Result<&ClaudeSettings, String> {
        match self {
            DriverStart::Claude(settings) => Ok(settings),
            DriverStart::Codex(_) => Err("the Codex turn driver has not landed yet".to_string()),
            DriverStart::OpenCode(_) => Err("OpenCode settings were paired with the Claude driver".to_string()),
        }
    }

    pub(crate) fn codex(&self) -> Result<&crate::config::CodexSettings, String> {
        match self {
            DriverStart::Codex(settings) => Ok(settings),
            DriverStart::Claude(_) => {
                Err("Codex settings were paired with the Claude driver".to_string())
            }
            DriverStart::OpenCode(_) => Err("OpenCode settings were paired with the Codex driver".to_string()),
        }
    }

    pub(crate) fn opencode(&self) -> Result<&crate::config::OpenCodeSettings, String> {
        match self {
            DriverStart::OpenCode(settings) => Ok(settings),
            _ => Err("OpenCode settings were paired with another driver".to_string()),
        }
    }
}

/// Send one turn, starting a session for the thread if it has none.
///
/// Synchronous and non-blocking: it is called from the socket's read loop, which
/// must be free to take the next frame. The failure it can return is the prompt
/// channel being full or closed, which means a session that is not consuming —
/// and that is worth telling the client about rather than dropping.
///
/// The registry choice is carried in [`Start::driver`], pairing one driver's
/// settings with its implementation. Everything after this match is written
/// against [`Driver`], so a second agent adds an arm here rather than another
/// session loop.
pub fn send(threads: &Threads, start: &Start, turn_id: String, text: String) -> Result<(), String> {
    let driving = threads.clone();
    let starting = start.clone();
    let prompts = match &start.driver {
        DriverStart::Claude(_) => {
            threads.attach(&start.thread_id, move |incoming, signals, epoch| {
                tokio::spawn(drive::<crate::turn::Claude>(
                    driving, starting, incoming, signals, epoch,
                ))
            })
        }
        DriverStart::Codex(_) => {
            threads.attach(&start.thread_id, move |incoming, signals, epoch| {
                tokio::spawn(drive::<crate::codex::Codex>(
                    driving, starting, incoming, signals, epoch,
                ))
            })
        }
        DriverStart::OpenCode(_) => {
            threads.attach(&start.thread_id, move |incoming, signals, epoch| {
                tokio::spawn(drive::<crate::opencode::OpenCode>(driving, starting, incoming, signals, epoch))
            })
        }
    };

    // The whole prompt is built here rather than by the caller, because what a
    // turn carries is more than what the developer typed: it also carries what
    // the conversation said the agent should be running under when this turn was
    // dispatched, and that is read off the same [`Start`] the session was opened
    // from. A session started a line above is already launched with those values
    // and the driver's guard makes it the no-op it should be; what this is for is
    // every turn after the first, where the launch flags were the only place they
    // had ever reached the child.
    let prompt = Prompt {
        turn_id,
        text,
        wanted: Retune {
            runtime_mode: start.runtime_mode.clone(),
            model: start.model.clone(),
        },
    };

    prompts.try_send(prompt).map_err(|error| match error {
        tokio::sync::mpsc::error::TrySendError::Full(_) => {
            "The agent has not read the turns already sent to it, so this one was not queued."
                .to_string()
        }
        tokio::sync::mpsc::error::TrySendError::Closed(_) => {
            "The agent session has ended and could not be sent this turn.".to_string()
        }
    })
}

/// The session: start an agent, feed it turns and decisions, publish what it
/// says, reap it.
///
/// Generic over the agent and over nothing else. Which one is running is decided
/// at the one call site in [`send`]; everything from here to the end of the
/// function is what every session owes every conversation, whichever agent
/// answered it.
///
/// `epoch` is which of the conversation's sessions this one is, and is given
/// back on the way out — a session may outlive its own slot, so it gives up only
/// the slot it was in. See [`crate::threads::Threads::detach`].
async fn drive<D: Driver>(
    threads: Threads,
    mut start: Start,
    mut prompts: tokio::sync::mpsc::Receiver<Prompt>,
    mut signals: tokio::sync::mpsc::Receiver<Signal>,
    epoch: u64,
) {
    let Opened {
        mut driver,
        decided: opened,
    } = match D::open(&start).await {
        Ok(opened) => opened,
        Err(why) => {
            // The session never existed, so there is no turn to attribute the
            // failure to beyond the one that asked for it. Reported in the
            // conversation rather than only to a log, because the developer is
            // looking at the conversation.
            threads.apply(
                &start.thread_id,
                Change::Activity(Activity::failed("session.failed", &why)),
            );
            threads.apply(
                &start.thread_id,
                Change::Session(Session {
                    status: SessionStatus::Error,
                    runtime_mode: start.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: Some(why),
                    updated_at: now_iso(),
                }),
            );
            threads.detach(&start.thread_id, epoch);
            return;
        }
    };
    spend(&threads, &start, opened);

    let mut driving = Driving {
        provider: start.provider.clone(),
        turn: None,
        outstanding: HashMap::new(),
        interrupts: 0,
        measurements: 0,
        retunes: 0,
        pushed: HashMap::new(),
        unmeasured: false,
        drift_reported: Drift::default(),
        finished: None,
        reported_usage: None,
    };
    // A turn that arrived while another was still running. Held rather than
    // sent: sending it would orphan the turn in flight — that turn would never
    // settle, and the finished one's duration and cost would be attributed to
    // the wrong turn.
    let mut waiting: Option<Prompt> = None;
    // False once the prompt channel has closed. The agent is then told there
    // will be no more turns and the loop keeps draining what it still owes.
    let mut accepting = true;
    // False once the signal channel has closed, which is the same moment — both
    // ends live on the thread's `Live`. Tracked separately anyway, because a
    // closed channel yields `None` forever and a `select!` arm that kept polling
    // one would spin.
    let mut listening = true;
    // True once the developer has ended the session. Read at the bottom of this
    // function, where the difference it makes is that a turn cut short by *this*
    // is not reported as the agent having died: they asked.
    let mut asked_to_stop = false;
    // A prompt write failed before the driver could begin the turn. Kept until
    // reaping so the shared ending reports the error without pretending the
    // agent had started work or asking either driver to duplicate this policy.
    let mut send_failure = None;

    'session: loop {
        // Anything already owed to the turn that just ended is dealt with before
        // the next one is started.
        //
        // The `select!` below cannot do this, because when a signal and a prompt
        // are both ready it takes either — and *which* matters here in a way it
        // does not anywhere else. A developer whose turn finished a moment ago
        // may have a stop click still in flight; the client sends one with no
        // `turnId`, meaning "whatever is running". Taken now it is the no-op it
        // was meant to be. Taken one iteration after the queued prompt, it stops
        // a turn that had just started, which is the developer's click landing on
        // the wrong turn.
        //
        // The window is real rather than theoretical: this loop `await`s a
        // `git add -A` at the end of every turn ([`checkpoint`]), and a click
        // during it is exactly a click after the turn the developer was watching
        // settled.
        while let Ok(signal) = signals.try_recv() {
            match signal {
                Signal::Answer(answered) => {
                    answer(&threads, &start, &mut driver, &mut driving, answered).await
                }
                Signal::AnswerUserInput(answered) => {
                    answer_user_input(&threads, &start, &mut driver, &mut driving, answered).await
                }
                Signal::Interrupt { turn_id } => {
                    interrupt(&threads, &start, &mut driver, &mut driving, turn_id).await
                }
                // Taken here as well as in the `select!` below, and it belongs
                // here most: the drain exists so that what was owed to the turn
                // that just ended is dealt with before the next one starts, and
                // a stop the developer pressed a moment ago must not be spent on
                // a turn they never saw begin.
                Signal::Stop => {
                    asked_to_stop = true;
                    break 'session;
                }
            }
        }

        // Whatever is waiting goes next, as soon as the turn before it is done.
        if accepting && driving.turn.is_none() {
            if let Some(prompt) = waiting.take() {
                // Before anything this turn publishes and before the turn itself
                // reaches the agent, which is what makes the mode this turn is
                // *requested* under the mode it is *answered* under. It moves
                // `start`, so the three session events below — `starting` from
                // the thread, `running` from here, and whatever ends the turn —
                // all report the same thing.
                //
                // **Here rather than when the prompt was taken off the channel**,
                // and that is what keeps a mode off the turn already in flight:
                // a turn queued behind a running one waits in `waiting` with its
                // own mode still attached, and spends it when its own turn comes.
                // Ticket 02's rule, and the spec's story 29.
                retune(&threads, &mut start, &mut driver, &mut driving, &prompt.wanted).await;

                // The turn is under way, and *this* is where the session enters
                // `running` — not the agent's `init` line, which a long-lived
                // child prints once for the whole conversation. Driving it off
                // `init` would leave every turn after the first in `starting`,
                // and a session that is not `running` settles the turn at the
                // first assistant message, which is the mid-turn settle the
                // client's reducer exists to avoid.
                //
                // **Before the baseline, not after**, and the difference is
                // seconds on a real repository. The client draws its working
                // indicator from this status, and the baseline below is a
                // `git add -A` over the whole project with no stat cache — a
                // fifth of a second on a scratch repo and over two on this one
                // (measured with `tools/ui-driver/first-turn.mjs`). Publishing
                // after it meant the developer pressed send and watched a pane
                // with nothing in it for as long as their repository is large,
                // which reads as the message having gone nowhere.
                //
                // Honest as well as kinder: the turn *is* running from here. The
                // developer asked for it, this loop has accepted it and is doing
                // the work that has to happen before the agent can be given it.
                // Nothing the client folds needs the prompt to have been written
                // yet, and a send that then fails settles the session on the way
                // out like any other failure.
                running(&threads, &start, &prompt.turn_id);

                // Before the agent is given the turn, which is the whole of what
                // makes it a *baseline*: everything the agent does from the next
                // line onwards is what this turn's diff will be against. Awaited
                // rather than spawned for the same reason — a capture racing the
                // agent's first edit would record a tree that already had it.
                baseline(&threads, &start).await;
                if let Err(error) = driver.send(&prompt.text).await {
                    eprintln!("laplus: cannot send a turn to the agent: {error}");
                    send_failure = Some(format!(
                        "The turn could not be sent to the agent: {error}"
                    ));
                    break;
                }
                driving.turn = Some(InFlight {
                    turn_id: prompt.turn_id.clone(),
                    assistant_message_id: None,
                    tools: HashMap::new(),
                    stopped: None,
                });
                continue;
            }
        }

        // The channel is polled whether or not a turn is running, so that a
        // shutdown mid-turn still closes the agent's input promptly. What it is
        // not allowed to do is take a second prompt before the first has been
        // dealt with, which is what `PROMPT_QUEUE` is behind it for.
        //
        // A signal is polled under no such condition, and that asymmetry is the
        // point: both kinds are owed to the turn *in flight*. A decision the
        // agent has stopped for, deferred behind a queued turn, is the deadlock
        // ticket 13 was about; an interrupt deferred the same way is a stop
        // button that works once the thing it was meant to stop has finished.
        //
        // The driver's arm is the one that has to be cancel-safe, because it is
        // the one dropped unfinished whenever another wins — see [`Driver::next`],
        // where that is a requirement rather than an observation.
        let next = tokio::select! {
            event = driver.next(&mut driving) => Next::Event(event),
            signal = signals.recv(), if listening => Next::Signal(signal),
            prompt = prompts.recv(), if accepting && waiting.is_none() => Next::Prompt(prompt),
        };

        match next {
            Next::Event(Some(mut decided)) => {
                // Taken before the changes are spent and applied after them, so
                // the developer reads the refusal and *then* the badge corrects
                // itself, rather than the badge flipping back with nothing
                // beside it saying why.
                let reverts = decided.reverts.take();
                spend(&threads, &start, decided);
                if let Some(refused) = reverts {
                    refused.revert(&mut start);
                    // Republished because the mode this turn was announced under
                    // is now known to be wrong, and the session is where the
                    // client reads it. Only while a turn is running: between
                    // turns there is no session event to correct, and the next
                    // one will carry the reverted value anyway.
                    if let Some(active) = driving.turn.as_ref().map(|turn| turn.turn_id.clone()) {
                        running(&threads, &start, &active);
                    }
                }
                // Asked before the checkpoint below, which is a `git add -A`
                // over the whole project: the meter is what the developer is
                // looking at and the question costs one write to the agent, so
                // it does not queue behind seconds of git.
                //
                // Asked *here* rather than by the driver on the line that
                // wanted it, and cancel-safety is the reason: a write inside
                // [`Driver::next`] is a write that can be dropped mid-await,
                // taking the event that occasioned it with it.
                if std::mem::take(&mut driving.unmeasured) {
                    measure(&mut driver, &mut driving).await;
                }
                // The turn is over, so what the agent left behind is what this
                // turn did. Recorded before the loop takes the next prompt,
                // which is what makes the next turn's baseline this checkpoint
                // rather than a tree somebody has since typed into.
                if let Some(finished) = driving.finished.take() {
                    checkpoint(&threads, &start, &finished).await;
                }
            }
            // The agent stopped producing: it exited, or its output was
            // abandoned. Either way there is nothing more to publish.
            Next::Event(None) => break,
            Next::Signal(Some(Signal::Answer(answered))) => {
                answer(&threads, &start, &mut driver, &mut driving, answered).await;
            }
            Next::Signal(Some(Signal::AnswerUserInput(answered))) => {
                answer_user_input(&threads, &start, &mut driver, &mut driving, answered).await;
            }
            Next::Signal(Some(Signal::Interrupt { turn_id })) => {
                interrupt(&threads, &start, &mut driver, &mut driving, turn_id).await;
            }
            // The session is over. Left by leaving the loop rather than by
            // closing the agent's input and draining, which is what the *channel
            // closing* means below: everything this function does after the loop
            // is the ending a session is owed, and [`Driver::stop`] at the end of
            // it reaps the child under a bound of its own. A drain would be a
            // gentler ending with no bound on it, and the case this command
            // exists for is an agent that is not answering.
            Next::Signal(Some(Signal::Stop)) => {
                asked_to_stop = true;
                break;
            }
            Next::Signal(None) => listening = false,
            Next::Prompt(Some(prompt)) if D::STEERS_ACTIVE_TURN && driving.turn.is_some() => {
                // OpenCode accepts another prompt while its session is busy.
                // It remains part of the active Laplus turn: no baseline,
                // retune, running event, or replacement InFlight is created.
                if let Err(error) = driver.send(&prompt.text).await {
                    eprintln!("laplus: cannot steer the agent: {error}");
                    threads.apply(
                        &start.thread_id,
                        Change::Activity(Activity::failed(
                            "turn.steer-failed",
                            &format!("The steer could not be sent to the agent: {error}"),
                        )),
                    );
                }
            }
            Next::Prompt(Some(prompt)) => waiting = Some(prompt),
            Next::Prompt(None) => {
                accepting = false;
                driver.close_input();
            }
        }
    }

    // A reply that streamed and will never be buffered, because the agent went
    // away in the middle of it. Two things are owed to it and neither is
    // optional:
    //
    // - **The client is showing it as still arriving.** A message left
    //   `streaming` is a reply the UI renders as growing, for the life of the
    //   thread, and after every restart — the same defect the ordinary
    //   reconcile is careful to avoid when a turn buffers nothing.
    // - **A delta owes the database nothing**, by design ([`crate::threads`]'s
    //   `durable`: a row per token would put the disk in the streaming path), so
    //   until a message settles there is nothing written down. Without this, the
    //   partial reply survives in memory and is gone the next time the app opens.
    //
    // Sent with **no text**, which is deliberate: the empty case is the one
    // where the accumulation stands rather than being replaced, so what the
    // developer saw stream is what is kept — and no reconciliation is recorded,
    // because none happened. A message forged out of the deltas and compared
    // against them would report the assumption as checked on a turn where
    // nothing checked it.
    if let Some(active) = driving.turn.as_mut() {
        if let Some(message_id) = active.assistant_message_id.take() {
            threads.apply(
                &start.thread_id,
                Change::AssistantMessage {
                    message_id,
                    turn_id: active.turn_id.clone(),
                    text: String::new(),
                },
            );
        }
    }

    // Whatever the agent was still waiting on is never going to be answered now,
    // and the client derives its pending-approval panel from these two kinds
    // alone — so a request left open here is a composer the developer cannot type
    // into, for the life of the conversation and across every restart after it.
    // Closing them is what makes "the session remains usable" true of the way a
    // session actually ends.
    //
    // A question is closed with *its* kinds, not these: the two folds are
    // separate (`derivePendingUserInputs` beside `derivePendingApprovals`), so an
    // `approval.resolved` over a question closes nothing and leaves exactly the
    // stuck composer this loop exists to prevent. Said as unanswerable rather
    // than as answers, because no answers were given and a row claiming some were
    // would put words in the developer's mouth.
    for asked in driving.take_outstanding() {
        let activity = match crate::worklog::questions(&asked).is_some() {
            true => crate::worklog::unanswerable_user_input(&asked.request_id),
            false => crate::worklog::resolved(
                &asked,
                Decision::Cancel,
                driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
            ),
        };
        threads.apply(&start.thread_id, Change::Activity(activity));
    }

    // Reaped, and the two sentences an ending can owe come back from the reaping.
    // Both quote the agent, so both are the driver's to write and neither can be
    // asked for earlier: what a `--resume` the agent would not honour said, and
    // what it said on the way out of a turn it died in the middle of. See
    // [`Driver::stop`], and [`Reaped`] for why the loop rather than the driver
    // decides what becomes of them.
    //
    // `asked_to_stop` goes in because it is the one thing the driver cannot know.
    // A turn cut short because the developer ended the session is not a turn the
    // agent died in the middle of: reporting it as a death would put an error on a
    // conversation whose only fault was being stopped, and would settle the turn as
    // `error` rather than as the `interrupted` a `stopped` session settles it to —
    // which is the same reading `interrupted` and `stopped` already share
    // (`CONTEXT.md`, *Settling*): from the turn's point of view it did not finish,
    // and nothing went wrong.
    let Reaped { refused, death } = driver.stop(&mut driving, asked_to_stop).await;
    if let Some(why) = &send_failure {
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed("session.failed", why)),
        );
    }
    if let Some(why) = &refused {
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed("session.resume-failed", why)),
        );
    }

    // Said in the conversation and not only on the session, because the session
    // is a banner and the conversation is what the developer is reading. A
    // refusal has already put its own row up, so this does not repeat it — and it
    // wins over the death when there is one, because "the agent stopped before
    // the turn finished" is true of a resume that was refused and says nothing
    // about why.
    if let (Some(why), None) = (&death, &refused) {
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed("session.failed", why)),
        );
    }

    // The slot given up before the ending is published, and **whether the
    // conversation is still this session's to describe decides whether the ending
    // is published at all.** A driver can outlive its own session: a developer who
    // stops one and sends a turn straight afterwards — which is exactly what the
    // branch toolbar does — has a new session describing this conversation while
    // this one is still being reaped. Its ending would then settle a turn that had
    // just started and report a session that had just opened as gone.
    //
    // The rows above are said either way, because they are about *this* session's
    // turn rather than about the conversation's current state: a partial reply
    // left streaming and a permission request left open are defects whoever is
    // running now.
    let ours = threads.detach(&start.thread_id, epoch);

    // A write can lose the race with a child refusing its resume and report a
    // broken pipe. The refusal is the cause the developer can act on; the write
    // failure is only its consequence, so it must not replace the driver's
    // provider-specific continuation explanation on the session.
    let failure = refused.or(send_failure).or(death);
    if ours {
        threads.apply(
            &start.thread_id,
            Change::Session(Session {
                // **`error` rather than `stopped` for a turn cut short**, and
                // ticket 15 settled it deliberately — see ADR-0004. `stopped` is
                // available and would settle the turn as `interrupted`; it is not
                // used, because nobody asked for this and because `lastError`
                // below — the only place the developer is told the agent went away
                // mid-turn — is the sentence a session that is not in `error` has
                // nowhere to put.
                status: match failure.is_some() {
                    true => SessionStatus::Error,
                    false => SessionStatus::Stopped,
                },
                runtime_mode: start.runtime_mode.clone(),
                active_turn_id: None,
                last_error: failure,
                updated_at: now_iso(),
            }),
        );
    }

    // A turn the agent died in the middle of still changed the working tree, and
    // the developer's first question about a session that fell over is what it
    // had already done. So this ending is checkpointed like any other — the only
    // difference is that nothing published a `turn.completed` for it, which is
    // why it cannot be caught by the loop above.
    //
    // As an `error`, because that is how the session above has just reported it:
    // ADR-0004 settles a turn cut short by a dead agent as an error rather than
    // an interruption, and a checkpoint saying anything else would move the turn
    // back — see `Ending::checkpoint_status` in [`crate::turn`].
    //
    // **A session the developer ended gets no row**, and that is the same rule
    // rather than an exception to it: there is no checkpoint status that means "the
    // developer ended the session", so every one this could send would relabel the
    // turn the moment the client folded it — `error` says the turn failed, and it
    // did not. What the turn had already done to the tree falls into the diff of
    // the turn that follows, which is what a model built on photographs does with
    // an unattributed change (ADR-0008).
    //
    // Nor does a session the conversation has already replaced, for the reason the
    // ending above is conditional: the client reads a checkpoint's status back
    // into the *latest* turn, so a row for a turn two sessions ago would relabel
    // the turn running now.
    if let Some(active) = driving
        .turn
        .take()
        .filter(|_| ours && !asked_to_stop)
    {
        checkpoint(
            &threads,
            &start,
            &Finished {
                turn_id: active.turn_id,
                status: "error",
            },
        )
        .await;
    }
}

/// Record what the working tree looks like before this conversation's next turn
/// is given to the agent.
///
/// Nothing to do in the ordinary case: the checkpoint taken at the end of the
/// previous turn *is* this turn's baseline, which is what makes a conversation's
/// checkpoints a chain rather than a set of pairs. The cases where there is
/// something to do are the first turn of a conversation, and the first turn
/// after the developer ran `vcs.init` on a project that had no repository — both
/// of which are "there is no tree recorded for turn zero yet".
async fn baseline(threads: &Threads, start: &Start) {
    let turn_count = threads.checkpoint_count(&start.thread_id);
    let reference = crate::checkpoints::reference(&start.thread_id, turn_count);
    let root = std::path::PathBuf::from(&start.workspace_root);

    let recorded = tokio::task::spawn_blocking(move || {
        if crate::checkpoints::present(&root, &reference) {
            return Ok(());
        }
        crate::checkpoints::capture(&root, &reference)
    })
    .await;

    // Logged and not said in the conversation, unlike the failure at the end of
    // a turn. A baseline that could not be written is only visible as the turn
    // that follows it having no diff, and *that* is the moment the developer is
    // told — saying it twice would put two rows in the work log for one problem.
    //
    // A `JoinError` is the blocking pool having gone, which is the runtime
    // shutting down. Nothing is owed to a conversation nobody is reading.
    if let Ok(Err(why)) = recorded {
        if !crate::git::is_not_a_repository(&why) {
            eprintln!(
                "laplus: cannot record the state of {} before a turn: {}",
                start.workspace_root,
                why.detail()
            );
        }
    }
}

/// Record what the working tree looks like now as this conversation's next
/// checkpoint, and publish the row that offers the turn for review.
///
/// **A project that is not a repository is not a failure.** There is nowhere to
/// keep a checkpoint and nothing the developer did wrong; `vcs.init` is the door
/// out, and until they walk through it a conversation simply has no turns to
/// diff. Every other refusal is said in the conversation, because a turn the
/// developer cannot review is a thing they will otherwise go looking for.
async fn checkpoint(threads: &Threads, start: &Start, finished: &Finished) {
    let previous = threads.checkpoint_count(&start.thread_id);
    let turn_count = previous + 1;
    let from = crate::checkpoints::reference(&start.thread_id, previous);
    let reference = crate::checkpoints::reference(&start.thread_id, turn_count);
    let root = std::path::PathBuf::from(&start.workspace_root);

    // On a blocking thread, like every other git in this server: it is a child
    // process over a repository whose size is the developer's, and a runtime
    // worker parked on one is a worker the socket is not using.
    let taken = tokio::task::spawn_blocking({
        let reference = reference.clone();
        move || {
            crate::checkpoints::capture(&root, &reference)?;
            // Best effort, and deliberately not fatal: the summary is the line
            // above the patch, and a turn whose tree was recorded is reviewable
            // whether or not this could be read.
            Ok(crate::checkpoints::changed(&root, &from, &reference).unwrap_or_default())
        }
    })
    .await;

    let files = match taken {
        Ok(Ok(files)) => files,
        Ok(Err(why)) => {
            if !crate::git::is_not_a_repository(&why) {
                threads.apply(
                    &start.thread_id,
                    Change::Activity(Activity::failed(
                        "checkpoint.failed",
                        &format!(
                            "The state of the project after this turn could not be recorded, so \
                             the turn has no diff to review: {}",
                            why.detail()
                        ),
                    )),
                );
            }
            return;
        }
        // The blocking pool is gone, which happens when the runtime is shutting
        // down. There is nothing to say to a conversation nobody is reading.
        Err(_) => return,
    };

    threads.apply(
        &start.thread_id,
        Change::Checkpointed(Box::new(crate::threads::Checkpoint {
            turn_id: finished.turn_id.clone(),
            turn_count,
            reference,
            status: finished.status,
            files,
            // Resolved by the fold, which is where the transcript is. See
            // [`crate::threads::Threads::fold`].
            assistant_message_id: None,
            completed_at: now_iso(),
        })),
    );
}

/// Which of the three sources the loop heard from. A named value rather than
/// bodies inside `select!`, because writing to the agent needs the same mutable
/// borrow the event future is holding until the select is over.
enum Next {
    Event(Option<Decided>),
    Prompt(Option<Prompt>),
    Signal(Option<Signal>),
}

/// What the session and its driver both need, and neither could hold alone.
///
/// Most of it is what a fold cannot know: a driver's fold knows a permission was
/// *asked*, and this knows it has not been answered and which turn to attribute
/// the answer to. The rest — `unmeasured` and `finished` — is the traffic the
/// other way, a driver saying that something has to happen where a session can
/// `await` it. Lent to [`Driver::next`] and to [`Driver::stop`] for both
/// reasons.
pub(crate) struct Driving {
    /// The provider instance whose opaque continuation the driver may replace.
    pub(crate) provider: crate::provider::ProviderIdentity,
    /// The turn the agent is currently working on.
    pub(crate) turn: Option<InFlight>,
    /// Permission requests published and not yet answered, by the id the client
    /// answers with.
    ///
    /// Per session rather than per turn, unlike the tool calls beside them, and
    /// for the opposite reason: a tool call that outlived its turn has nothing
    /// left to answer it, while a permission that outlives its turn is a *panel
    /// the developer is still looking at*. Settling one is a thing that has to
    /// happen, so it must not be dropped with the turn.
    pub(crate) outstanding: HashMap<String, ApprovalRequest>,
    /// How many interrupts this session has sent, which is what the next one's
    /// id is minted from.
    ///
    /// Per session and monotonic, so an acknowledgement can be matched to the
    /// request it answers and a *late* one — for a turn that has already ended —
    /// is recognised as late rather than mistaken for the current one.
    pub(crate) interrupts: usize,
    /// How much of this session's drift the developer has already been shown.
    ///
    /// The counters are the session's and monotonic, so what a report has to
    /// carry is the *difference* — a running total repeated on every turn would
    /// say the format moved on a turn where nothing did, which is the noise that
    /// trains a reader to skip the turn where it had.
    ///
    /// Kept here rather than on the turn, and that is the difference between
    /// "what this turn failed to read" and "what has gone unreported": the CLI
    /// talks *between* turns as well as during them — a rate-limit notice, a
    /// compaction boundary — and drift there belongs to somebody. Anchoring it
    /// to the start of a turn would drop it.
    pub(crate) drift_reported: Drift,
    /// How many times this session has asked the agent how full its window is,
    /// so the next request gets an id nothing else has used.
    pub(crate) measurements: u32,
    /// How many mode and model pushes this session has sent, on the same terms.
    pub(crate) retunes: u32,
    /// Pushes written to the agent and not yet answered, by the id this server
    /// minted.
    ///
    /// The answer is the only thing that says a push landed, and a refusal has to
    /// put [`Start`] back — so what it was before travels here rather than being
    /// re-derived from a thread that has since moved on. See [`Pushed`].
    pub(crate) pushed: HashMap<String, Pushed>,
    /// A moment has arrived that only the agent can settle, and the loop has not
    /// asked it yet.
    ///
    /// The same one-line handoff from the fold to the loop that `finished` is,
    /// and for the same reason: asking is a *write* to the agent and so has to
    /// happen where the loop can `await` it, while noticing that the moment
    /// arrived happens where the lines are read, which is synchronous.
    pub(crate) unmeasured: bool,
    /// A turn that has just ended and whose working tree has not been recorded
    /// yet.
    ///
    /// A one-line handoff from the driver to the loop, and it exists because the
    /// two halves cannot swap places. Recording a checkpoint is a `git` and
    /// therefore has to happen where the loop can `await` it; noticing that a
    /// turn ended happens where the agent's events are read, which is
    /// synchronous. Written by the driver and taken by the loop on the next
    /// statement, so it is never carried across an iteration.
    ///
    /// **Left here rather than moved onto [`Decided`]**, along with `unmeasured`
    /// above. Both are already what that type is — a thing decided on one event
    /// and spent on the next — but both are read by the loop itself rather than
    /// by [`spend`], and neither is a change to a conversation. Moving them would
    /// have widened `Decided` from "what happened to the thread" to "what the
    /// session does next" for no assertion a driver's own tests could not already
    /// make: they are `&mut Driving` fields, and a test of a translation reads
    /// them off the `Driving` it handed in.
    pub(crate) finished: Option<Finished>,
    /// The last context-window reading the client was told about.
    ///
    /// Kept so an unchanged reading is not published again. The CLI repeats its
    /// counts — the `result` at the end of a turn usually agrees with the last
    /// assistant message of that turn — and a row per repetition would be a row
    /// per message on the thread, each one saying what the one before it said.
    /// Per session rather than per turn, because the repetition crosses the
    /// boundary: a turn that asked the agent nothing new ends where the last one
    /// did.
    pub(crate) reported_usage: Option<TokenUsage>,
}

/// A turn that ended in a way a checkpoint can describe.
///
/// The status is one of the contract's own, and which of them a turn earned is
/// the driver's to say — see `Ending::checkpoint_status` in [`crate::turn`] for
/// why *not saying* is one of the answers. The one exception is written by the
/// loop rather than by a driver, at the bottom of [`drive`]: a turn the agent
/// died in the middle of is an `error` by ADR-0004, and no driver is left to say
/// so.
pub(crate) struct Finished {
    pub(crate) turn_id: String,
    pub(crate) status: &'static str,
}

impl Driving {
    /// The id for the next interrupt, and the count that makes it unique.
    fn next_interrupt_id(&mut self) -> String {
        self.interrupts += 1;
        format!("interrupt-{}", self.interrupts)
    }

    /// The id for the next context-window question.
    ///
    /// Distinct from an interrupt's for the developer reading a log rather than
    /// for the code: the answer is told apart by what it carries, not by the id
    /// it names — see [`crate::protocol::Acknowledgement::reading`].
    fn next_measurement_id(&mut self) -> String {
        self.measurements += 1;
        format!("context-{}", self.measurements)
    }

    /// The id for the next mode or model push.
    ///
    /// One counter for both, because they are one kind of request as far as the
    /// answer is concerned: the id is what says *which* push a refusal is about,
    /// and [`Pushed`] carries which kind it was.
    fn next_retune_id(&mut self) -> String {
        self.retunes += 1;
        format!("retune-{}", self.retunes)
    }

    /// What has gone unread since the last time anybody was told, and a note
    /// that they have now been told it.
    ///
    /// Given the driver's running total rather than the driver's state: what has
    /// drifted is a count either way, and taking the count is what keeps this
    /// side of the pair from having to know whose fold produced it.
    pub(crate) fn drift_to_report(&mut self, total: Drift) -> Drift {
        let unreported = total.since(self.drift_reported);
        self.drift_reported = total;
        unreported
    }

    /// The context-window row the client has not been told yet, if the reading
    /// has moved, and a note that it has now been told.
    ///
    /// `None` when nothing has changed, which is most events: the counts arrive
    /// on every assistant message and on the `result`, and the last two of those
    /// usually agree.
    pub(crate) fn usage_to_report(&mut self, reading: Option<TokenUsage>) -> Option<TokenUsage> {
        let reading = reading?;
        if self.reported_usage.as_ref() == Some(&reading) {
            return None;
        }
        self.reported_usage = Some(reading.clone());
        Some(reading)
    }

    /// Take every request still waiting, so the caller can close it.
    fn take_outstanding(&mut self) -> Vec<ApprovalRequest> {
        let mut open: Vec<ApprovalRequest> = std::mem::take(&mut self.outstanding)
            .into_values()
            .collect();
        // A map has no order and these become rows in a work log. Ordered by the
        // id the agent minted, which is at least the same every run.
        open.sort_by(|left, right| left.request_id.cmp(&right.request_id));
        open
    }
}

/// Tell the agent what the developer decided, and say so in the conversation.
///
/// The agent is written to *first*, because that is the half with something
/// waiting on it: a decision published but not delivered would close the panel
/// on a conversation that is still stopped. If the write fails the row still goes
/// up — as a failure — because the alternative is a panel the developer can never
/// clear.
async fn answer<D: Driver>(
    threads: &Threads,
    start: &Start,
    driver: &mut D,
    driving: &mut Driving,
    answered: Answered,
) {
    // An id this session never asked about, or asked about and has already been
    // answered on. Said the one way the client recognises as "this request is
    // gone", so a panel left behind by a session that died without settling is
    // cleared by the first attempt to answer it rather than being permanent.
    let Some(asked) = driving.outstanding.remove(&answered.request_id) else {
        threads.apply(
            &start.thread_id,
            Change::Activity(crate::worklog::unanswerable(&answered.request_id)),
        );
        return;
    };

    let sent = driver.answer(&asked, Reply::Decided(answered.decision)).await;

    // "Cancel" is a denial that also stops the turn, wherever the driver puts
    // that — which makes it an interrupt this server did not send but did cause,
    // and the turn has to end the same way one it did send would. Ticket 13 sent
    // the decision correctly and left this half undone by name; it is the same
    // fact recorded in the same field, and it is recorded only if the decision
    // actually reached the agent.
    let mut cancelled = None;
    if sent.is_ok() && answered.decision == Decision::Cancel {
        if let Some(active) = driving.turn.as_mut() {
            if active.stop(&asked.request_id) {
                cancelled = Some(active.turn_id.clone());
            }
        }
    }

    let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());

    // Reported as a *session* failure rather than as an unanswerable request:
    // the request was real and this server knew it, and what went wrong is that
    // the agent stopped reading. The resolution below closes the panel either
    // way, which is why this does not have to.
    if let Err(error) = sent {
        eprintln!("laplus: cannot answer a permission request: {error}");
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed(
                "session.failed",
                &format!(
                    "The decision could not be sent to the agent, which is no longer reading: \
                     {error}"
                ),
            )),
        );
    }

    // Published either way, and after the write either way. This is what closes
    // the panel, and a decision that reached the agent and was never recorded
    // would leave the developer asked a second time about work already under way.
    threads.apply(
        &start.thread_id,
        Change::Activity(crate::worklog::resolved(
            &asked,
            answered.decision,
            turn_id,
        )),
    );

    // After the resolution, so the work log reads in the order it happened: the
    // developer answered the question, and answering it that way stopped the turn.
    if let Some(turn_id) = cancelled {
        stopped(threads, start, &turn_id, &asked.request_id);
    }
}

/// Tell the agent what the developer answered, and say so in the conversation.
///
/// [`answer`]'s twin for `AskUserQuestion`, and the same shape for the same
/// reasons: the agent is written to first because it is the half that has stopped,
/// and the row goes up either way because the alternative is a question header the
/// developer can never clear.
///
/// Two things it does *not* have. There is no cancel — the composer offers no way
/// to refuse a question, so no answer here can end a turn. And there is no
/// decision to refuse: [`crate::worklog::Decision::parse`] guards the approval
/// path against a verb this server cannot read, while answers are the developer's
/// own text and are carried through unread.
async fn answer_user_input<D: Driver>(
    threads: &Threads,
    start: &Start,
    driver: &mut D,
    driving: &mut Driving,
    answered: UserInputAnswered,
) {
    // An id this session never asked about, or has already been answered on.
    // Closed with the wording the client folds as "that question is gone", which
    // is what clears a header left behind by a session that died holding one.
    let Some(asked) = driving.outstanding.remove(&answered.request_id) else {
        threads.apply(
            &start.thread_id,
            Change::Activity(crate::worklog::unanswerable_user_input(
                &answered.request_id,
            )),
        );
        return;
    };

    let sent = driver
        .answer(&asked, Reply::Answers(&answered.answers))
        .await;

    if let Err(error) = sent {
        eprintln!("laplus: cannot answer a question: {error}");
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed(
                "session.failed",
                &format!(
                    "The answers could not be sent to the agent, which is no longer reading: \
                     {error}"
                ),
            )),
        );
    }

    threads.apply(
        &start.thread_id,
        Change::Activity(crate::worklog::user_input_resolved(
            &asked.request_id,
            &answered.answers,
            driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
        )),
    );
}

/// Stop the turn the agent is working on, and say so in the conversation.
///
/// Three things make this a no-op rather than an error, and each is a race the
/// developer cannot see and should not have to think about:
///
/// - **Nothing is in flight.** The turn ended while the click was travelling, or
///   there was never one. Either way the agent is not doing the thing they asked
///   it to stop doing, which is what they wanted.
/// - **The turn named is not the turn running.** The client names the turn it is
///   looking at (`buildThreadTurnInterruptInput`), and the same race makes that
///   the *previous* turn a moment later. Stopping the one after it would be
///   stopping work the developer never saw start.
/// - **This turn has already been stopped.** A second click, or a stop after a
///   permission was cancelled. The agent is already winding down and a second
///   request would produce a second row saying so.
///
/// What is *not* published here is a session change. The turn is not over until
/// the agent says it is — it still owes whatever it had buffered, and then its
/// account of how the turn ended — and this server saying "stopped" before that
/// would be describing an ending it had only asked for. The [`Settles`] a driver
/// answers with is where the turn settles, and [`InFlight::stopped`] is what
/// tells it how.
async fn interrupt<D: Driver>(
    threads: &Threads,
    start: &Start,
    driver: &mut D,
    driving: &mut Driving,
    wanted: Option<String>,
) {
    // Every reason to do nothing is settled before an id is minted, so a no-op
    // costs nothing and the ids in the work log count stops that happened.
    let Some(active) = driving.turn.as_ref() else {
        return;
    };
    if wanted.is_some_and(|turn_id| turn_id != active.turn_id) || active.stopped.is_some() {
        return;
    }
    let turn_id = active.turn_id.clone();
    let request_id = driving.next_interrupt_id();

    // The agent is written to before anything else happens, the same way a
    // decision is and for the same reason: the rows below are what the developer
    // sees and the write is what actually stops the work. Publishing first would
    // be a claim about an agent still going — and *recording* first would be
    // worse, because a turn marked stopped that nobody managed to stop would
    // report itself as stopped when it finished normally.
    //
    // The borrow is taken again afterwards rather than held across the write:
    // `agent` and `driving` are separate borrows and the second must not outlive
    // the await, which is the same reason [`Next`] exists.
    if let Err(error) = driver.interrupt(&request_id).await {
        eprintln!("laplus: cannot interrupt the agent: {error}");
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed(
                "turn.interrupt-failed",
                &format!(
                    "The agent could not be asked to stop, because it is no longer reading: \
                     {error}"
                ),
            )),
        );
        return;
    }

    // The turn can only have gone away while the write was in flight if the
    // agent's output ended, and then the loop is about to end too.
    let Some(active) = driving.turn.as_mut() else {
        return;
    };
    active.stop(&request_id);
    stopped(threads, start, &turn_id, &request_id);
}

/// Ask the agent how full its context window is.
///
/// **Nothing is published, and a failure is not reported.** That is the whole
/// difference between this and every other write to the agent, and it is what
/// makes the request safe to add to a build whose CLI may not implement it: a
/// stop that did not land is a button that did nothing and has to be said, while
/// a question that did not land leaves the meter exactly as it was — filled from
/// the token counts, the way ticket 40 filled it. A row reading "the agent would
/// not say how full its context window is" would be the first thing a developer
/// saw on an older CLI, about a number already on screen.
///
/// The answer comes back on stdout whenever the agent gets to it, and
/// [`crate::protocol::SessionState::reduce`] folds it. Nothing here waits, and
/// no state records that a question is outstanding — a second question asked
/// before the first was answered is two readings rather than a problem, and the
/// later of them wins because it arrives later.
async fn measure<D: Driver>(driver: &mut D, driving: &mut Driving) {
    let request_id = driving.next_measurement_id();
    if let Err(error) = driver.measure(&request_id).await {
        eprintln!("laplus: cannot ask the agent how full its context window is: {error}");
    }
}

/// Move the child onto the mode and model this turn asks for, before it is given
/// the turn.
///
/// The whole of ticket 11 of `thread-lifecycle`. An agent reads its latitude when
/// it is started and one process serves a whole conversation — so before this a
/// developer who tightened `full-access` to `approval-required` saw the picker
/// move, saw it survive a restart, saw the next turn *requested* under the new
/// mode, and was answered by an agent still bypassing permissions.
///
/// Four rules, and each of them is an acceptance criterion:
///
/// - **Nothing is sent for a value that has not moved.** The comparison is
///   against [`Start`], which is what the child is running under rather than what
///   the thread last said — so a conversation whose mode never changes never
///   sends one of these, and upstream's guard-on-change is followed rather than
///   re-derived.
/// - **The capture moves only when the write did.** `start` is what every session
///   event reads, so setting it before the write happened would publish a claim
///   about a child nobody had managed to tell.
/// - **The refusal is left to the agent's answer.** Whether a push landed is
///   something the agent says later, under the same `request_id`, and a driver
///   reports it by putting a [`Pushed`] on [`Decided::reverts`] — which the loop
///   spends by putting `start` back. What is reported *here* is the narrower
///   failure of the child no longer reading at all.
/// - **A model is pushed only when there is one to push.** A selection that names
///   none cannot be expressed as a request — there is nothing that means "go back
///   to your default" — so the child keeps what it has, and `start` keeps saying
///   so.
async fn retune<D: Driver>(
    threads: &Threads,
    start: &mut Start,
    driver: &mut D,
    driving: &mut Driving,
    wanted: &Retune,
) {
    // Decided before anything is written, so "has it moved" is asked of the same
    // capture both questions are asked of — and so the loop below is one shape
    // rather than two nearly-identical ones.
    let mode = (wanted.runtime_mode != start.runtime_mode).then(|| Pushed::Mode {
        previous: start.runtime_mode.clone(),
        asked: wanted.runtime_mode.clone(),
    });
    let model = wanted
        .model
        .clone()
        .filter(|wanted| Some(wanted) != start.model.as_ref())
        .map(|asked| Pushed::Model {
            previous: start.model.clone(),
            asked,
        });

    for asked in [mode, model].into_iter().flatten() {
        let request_id = driving.next_retune_id();
        match driver.retune(&request_id, &asked).await {
            Ok(()) => {
                asked.apply(start);
                driving.pushed.insert(request_id, asked);
            }
            Err(error) => unreachable_agent(threads, start, &asked, &error),
        }
    }
}

/// The agent stopped reading before it could be told what this turn wants.
///
/// Reported rather than swallowed, unlike [`measure`]'s failure and for the
/// reason a failed interrupt is: the developer changed something and was shown
/// the change, so a change that reached nothing has to be said. The turn goes on
/// regardless — refusing to send it would leave a conversation with a message in
/// it and nothing running, which is worse than a turn answered under the old
/// rules with a row saying so.
fn unreachable_agent(threads: &Threads, start: &Start, asked: &Pushed, error: &std::io::Error) {
    let what = asked.what();
    eprintln!("laplus: cannot tell the agent its {what}: {error}");
    threads.apply(
        &start.thread_id,
        Change::Activity(Activity::failed(
            "session.retune-failed",
            &format!(
                "The agent could not be told which {what} this turn wants, because it is no \
                 longer reading: {error}"
            ),
        )),
    );
}

/// One thing this server asked a running child to become, and what it was before.
///
/// Held from the moment the request is written until the agent answers it, because
/// the answer is the only thing that says whether it landed — and a refusal has
/// to put [`Start`] back to the value the child is really running under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pushed {
    Mode { previous: String, asked: String },
    Model { previous: Option<String>, asked: String },
}

impl Pushed {
    /// Which of the two this is, in the words the developer's own picker uses.
    ///
    /// Read off the variant rather than passed alongside it, so there is one
    /// place that decides what a mode and a model are called.
    pub(crate) fn what(&self) -> &'static str {
        match self {
            Pushed::Mode { .. } => "runtime mode",
            Pushed::Model { .. } => "model",
        }
    }

    /// Take the driver's capture to what the child was just asked to become.
    ///
    /// Called only where the write succeeded, which is the whole of the rule:
    /// [`Start`] is what every session event reads, so moving it before the line
    /// was written would publish a claim about a child nobody had told.
    pub(crate) fn apply(&self, start: &mut Start) {
        match self {
            Pushed::Mode { asked, .. } => start.runtime_mode = asked.clone(),
            Pushed::Model { asked, .. } => start.model = Some(asked.clone()),
        }
    }

    /// What the developer is told, when the CLI would not take it.
    ///
    /// Names what was refused, in this server's own vocabulary rather than the
    /// CLI's: the developer picked `approval-required` in a menu and has never
    /// seen the word `default`.
    pub(crate) fn sentence(&self, why: &str) -> String {
        match self {
            Pushed::Mode { asked, previous } => format!(
                "The agent would not change to the '{asked}' runtime mode and is still running \
                 under '{previous}': {why}"
            ),
            Pushed::Model { asked, previous } => match previous {
                Some(previous) => format!(
                    "The agent would not change to the '{asked}' model and is still running \
                     '{previous}': {why}"
                ),
                None => format!(
                    "The agent would not change to the '{asked}' model and is still running the \
                     one it started with: {why}"
                ),
            },
        }
    }

    /// Put the driver's capture back to what the child is really running under.
    pub(crate) fn revert(self, start: &mut Start) {
        match self {
            Pushed::Mode { previous, .. } => start.runtime_mode = previous,
            Pushed::Model { previous, .. } => start.model = previous,
        }
    }
}

/// Say in the conversation that the developer stopped this turn.
///
/// Two changes rather than one, because they are read by different parts of the
/// client and neither can be derived from the other:
///
/// - **The event settles the turn.** `thread.turn-interrupt-requested` is the
///   contract's own, and the client's reducer moves the latest turn to
///   `interrupted` on it *immediately* — so the turn stops being reported as
///   running when the developer's click lands rather than when the agent gets
///   round to admitting it. Without it the composer would show work in progress
///   for as long as the agent took to wind down.
/// - **The row is the record.** The work log is what a developer reads later,
///   and "this turn stopped because I stopped it" is not derivable from the
///   partial reply above it.
///
/// Shared by the two things that stop a turn — the stop button, and cancelling a
/// permission — because they are the same event to a client either way.
fn stopped(threads: &Threads, start: &Start, turn_id: &str, request_id: &str) {
    threads.apply(
        &start.thread_id,
        Change::InterruptRequested {
            turn_id: turn_id.to_string(),
        },
    );
    threads.apply(
        &start.thread_id,
        Change::Activity(Activity::info(
            "turn.interrupted",
            "The developer stopped the turn.",
            json!({
                "requestId": request_id,
                "turnId": turn_id,
                // What the client's work log renders as the row's body. Without
                // it the row is a heading with nothing under it — see
                // `Activity::failed`, which repeats its summary for the same
                // reason.
                "detail": "The developer stopped the turn. Anything the agent had already \
                           said is kept.",
            }),
            Some(turn_id.to_string()),
        )),
    );
}

/// The session is working on this turn.
fn running(threads: &Threads, start: &Start, turn_id: &str) {
    threads.apply(
        &start.thread_id,
        Change::Session(Session {
            status: SessionStatus::Running,
            runtime_mode: start.runtime_mode.clone(),
            active_turn_id: Some(turn_id.to_string()),
            last_error: None,
            updated_at: now_iso(),
        }),
    );
}

/// The turn the agent is currently working on.
pub(crate) struct InFlight {
    pub(crate) turn_id: String,
    /// Minted at the first piece of assistant text and cleared when that message
    /// completes, so a turn that produces several messages — commentary between
    /// tool calls — gives each its own id rather than appending them all into one.
    pub(crate) assistant_message_id: Option<String>,
    /// Tool calls announced and not yet answered, by the id the agent minted.
    ///
    /// The pairing has to be remembered because the two halves arrive in
    /// different messages and only the first one says what the tool was: a
    /// `tool_result` carries an id, a payload and nothing else. Per turn rather
    /// than per session, because a call is always answered within the turn that
    /// made it — and a turn that ended with one outstanding has nothing left to
    /// answer it.
    pub(crate) tools: HashMap<String, Call>,
    /// The id of the request that stopped this turn, once something has.
    ///
    /// Two things can be: an interrupt this server sent, or a permission the
    /// developer *cancelled*, which is a denial the CLI honours by stopping the
    /// turn. They are one field because they are one fact — the turn is ending
    /// because the developer said so — and the turn's ending has to read the
    /// same way whichever of them it was.
    ///
    /// Two things read it. An acknowledgement matches against it, so an answer
    /// to some other request is not taken for this one. And `Folded::Completed`
    /// reads it as a fact about *how* the turn ended: the CLI reports an aborted
    /// turn as an error, and a turn the developer stopped on purpose is not one
    /// to show them a failure for.
    pub(crate) stopped: Option<String>,
}

impl InFlight {
    /// Record that something has stopped this turn, and say whether it was the
    /// first thing to.
    ///
    /// `false` is a turn already stopping — a second click, or a stop after a
    /// permission was cancelled — and the caller answers it by saying nothing,
    /// because the work log already says the turn is being stopped.
    pub(crate) fn stop(&mut self, request_id: &str) -> bool {
        if self.stopped.is_some() {
            return false;
        }
        self.stopped = Some(request_id.to_string());
        true
    }

    /// Is this the answer to the stop this turn is waiting on?
    pub(crate) fn awaiting(&self, request_id: &str) -> bool {
        self.stopped.as_deref() == Some(request_id)
    }

    /// The agent will not stop, so this turn is going to end the way it was
    /// going to end.
    ///
    /// The flag has to come off rather than merely being ignored: a normal
    /// ending reported as one the developer asked for would be a work log
    /// claiming they did something they did not.
    pub(crate) fn carries_on(&mut self) {
        self.stopped = None;
    }

    pub(crate) fn was_stopped(&self) -> bool {
        self.stopped.is_some()
    }
}

/// What one thing the agent said turned out to mean for the conversation.
///
/// **This is the vocabulary a driver answers in**, and the reason the loop above
/// can be written once: a driver folds its own protocol and hands back one of
/// these, in the contract's own [`Change`]s, with nothing agent-shaped left in
/// it. [`spend`] applies it.
///
/// Deciding and applying are two steps for `docs/adr/0027`'s reason, which is
/// ADR-0025's taken one level down: a function that applies its own results can
/// only be tested by watching what it did to a live world, and this one's world
/// is a [`Threads`] with a running agent behind it.
///
/// **Three fields rather than a `Vec<Change>`**, because two of the things a
/// translation does are not changes:
///
/// - the event that ends a turn is published *only if the session is still
///   describing that turn*, which is a question about the world asked between two
///   applies. Returned as a precondition rather than answered by the driver, so
///   the lock it costs is taken on the events that end a turn rather than on
///   every one.
/// - a push the agent refused has to put [`Start`] back, which is not something a
///   conversation can be told, and the re-publish that follows it needs a value
///   the driver does not hold. So the refusal's *row* travels in `changes` and
///   the correction travels in `reverts`, and the loop spends both.
#[derive(Debug, Default)]
pub(crate) struct Decided {
    /// The changes to apply, in the order they were decided — which is the order
    /// the developer saw the work happen.
    pub(crate) changes: Vec<Change>,
    /// Opaque continuation data minted by the driver and published to nobody.
    pub(crate) provider_resume_cursor: Option<crate::provider::ResumeCursor>,
    /// The turn ended, and how. `None` on everything that did not end one.
    pub(crate) settles: Option<Settles>,
    /// A push the agent refused, and what this session's capture of itself has to
    /// go back to.
    pub(crate) reverts: Option<Pushed>,
}

/// The end of a turn, as whatever ended it reports it.
///
/// Two of the five fields of a [`Session`] and not the other three, and the split
/// is the seam rather than an arbitrary cut: **how the turn went** is what the
/// driver decided, while **which session and when** are facts the loop has held
/// since it started the child. Leaving the second pair to [`spend`] is what takes
/// the last clock read out of a translation.
#[derive(Debug)]
pub(crate) struct Settles {
    /// The turn that ended — and the turn the session must *still* be describing
    /// for the ending to be published at all.
    ///
    /// `None` is an ending that arrived with no turn in flight, and it is a
    /// value rather than a missing one: the session's `activeTurnId` is then also
    /// `None`, and the two matching is what makes this publishable.
    pub(crate) turn_id: Option<String>,
    pub(crate) status: SessionStatus,
    pub(crate) last_error: Option<String>,
}

/// Apply what a driver decided.
///
/// The impure half of the pair, and it is deliberately this short: every line of
/// it is a call on [`Threads`], so there is nothing here to get wrong that a test
/// of the driver's own translation would not already have caught.
///
/// The order is the order the driver decided in, and the two things that are not
/// changes go where they went before the split — the id after the row that may
/// have preceded it on the same event, the ending last of all.
pub(crate) fn spend(threads: &Threads, start: &Start, decided: Decided) {
    for change in decided.changes {
        threads.apply(&start.thread_id, change);
    }

    if let Some(cursor) = &decided.provider_resume_cursor {
        if cursor.provider == start.provider {
            threads.remember_provider_resume_cursor(&start.thread_id, cursor);
        }
    }

    let Some(settles) = decided.settles else { return };
    // The question [`Settles::turn_id`] exists to ask, and the lock it costs is
    // taken here — on the events that end a turn — rather than on every one the
    // agent produces.
    if threads.active_turn(&start.thread_id) != settles.turn_id {
        return;
    }

    threads.apply(
        &start.thread_id,
        Change::Session(Session {
            status: settles.status,
            // The session's own, not the ending's: a turn ends with the latitude
            // the child was started with, whatever the thread has been moved to
            // since. See `CONTEXT.md`, *Runtime mode*.
            runtime_mode: start.runtime_mode.clone(),
            active_turn_id: None,
            last_error: settles.last_error,
            updated_at: now_iso(),
        }),
    );
}

/// The thread and project a turn needs, gathered into what a session takes.
///
/// A free function rather than a method on either, because it is the one place
/// three things meet: the thread says which model and how much latitude, the
/// project says where, and the settings say which binary.
pub fn prepare(thread: &Thread, settings: &Settings) -> Result<PreparedDriver, String> {
    let instance = crate::provider::resolve_instance(
        settings,
        &thread.provider.instance_id,
        Some(&thread.provider.driver),
    ).map_err(|unavailable| match unavailable {
        crate::provider::InstanceUnavailable::Unknown => format!(
            "Provider instance '{}' is not registered, so thread '{}' cannot start a turn.",
            thread.provider.instance_id, thread.id
        ),
        crate::provider::InstanceUnavailable::Disabled => format!(
            "Provider instance '{}' is disabled, so thread '{}' cannot start a turn.",
            thread.provider.instance_id, thread.id
        ),
        crate::provider::InstanceUnavailable::Mismatched { configured, recorded } => format!(
            "Provider instance '{}' is registered for driver '{}', but thread '{}' records '{}'.",
            thread.provider.instance_id, configured, thread.id, recorded
        ),
    })?;
    let identity = instance.identity().clone();
    let driver = match instance {
        crate::provider::ConfiguredInstance::Claude(instance) => {
            DriverStart::Claude(instance.settings)
        }
        crate::provider::ConfiguredInstance::Codex(instance) => {
            DriverStart::Codex(instance.settings)
        }
        crate::provider::ConfiguredInstance::OpenCode(instance) => DriverStart::OpenCode(instance.settings),
    };

    Ok(PreparedDriver { identity, driver })
}

pub fn starting(thread: &Thread, workspace_root: &str, prepared: PreparedDriver) -> Start {
    debug_assert_eq!(prepared.identity.instance_id, thread.provider.instance_id);
    debug_assert_eq!(prepared.identity.driver, thread.provider.driver);

    Start {
        thread_id: thread.id.clone(),
        workspace_root: workspace_root.to_string(),
        model: thread.model(),
        runtime_mode: thread.runtime_mode.clone(),
        resume_cursor: thread
            .provider_resume_cursor
            .clone()
            .filter(|cursor| cursor.provider == thread.provider),
        provider: thread.provider.clone(),
        driver: prepared.driver,
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::broadcast;

    fn thread() -> Thread {
        let provider = crate::provider::registration(crate::provider::CLAUDE_DRIVER)
            .expect("registered")
            .identity(crate::provider::CLAUDE_INSTANCE_ID);
        Thread {
            id: "thread-1".to_string(),
            project_id: "project-1".to_string(),
            title: "Cursor test".to_string(),
            provider,
            model_selection: json!({"instanceId": "claudeAgent", "model": "claude-opus-5"}),
            runtime_mode: "full-access".to_string(),
            interaction_mode: "default".to_string(),
            branch: None,
            worktree_path: None,
            created_at: "2026-08-01T00:00:00.000Z".to_string(),
            updated_at: "2026-08-01T00:00:00.000Z".to_string(),
            messages: Vec::new(),
            activities: Vec::new(),
            checkpoints: Vec::new(),
            session: None,
            latest_turn: None,
            latest_user_message_at: None,
            provider_resume_cursor: None,
            lifecycle: crate::threads::Lifecycle::default(),
        }
    }

    #[test]
    fn a_driver_cursor_crosses_the_shared_boundary_without_socket_changes() {
        let (shell, mut announcements) = broadcast::channel(8);
        let threads = Threads::new(
            crate::store::Sequences::from(0),
            shell,
            crate::transcripts::Transcripts::nowhere(),
        );
        let thread = thread();
        threads.create(thread.clone()).expect("created");
        announcements.try_recv().expect("creation announcement");
        let before = threads.get("thread-1").expect("thread").to_detail_value();
        let cursor = crate::provider::ResumeCursor {
            provider: thread.provider.clone(),
            value: json!({"version": 1, "sessionId": "opaque-upstream"}),
        };
        let start = Start {
            thread_id: thread.id.clone(),
            workspace_root: "/work".to_string(),
            model: thread.model(),
            runtime_mode: thread.runtime_mode.clone(),
            resume_cursor: None,
            provider: thread.provider.clone(),
            driver: DriverStart::Claude(crate::config::ClaudeSettings {
                enabled: true,
                binary_path: "claude".to_string(),
                home_path: String::new(),
                launch_args: String::new(),
                custom_models: Vec::new(),
            }),
        };

        spend(&threads, &start, Decided { provider_resume_cursor: Some(cursor.clone()), ..Default::default() });

        let after = threads.get("thread-1").expect("thread");
        assert_eq!(after.provider_resume_cursor, Some(cursor));
        assert_eq!(after.to_detail_value(), before, "transcript/socket state changed");
        assert!(announcements.try_recv().is_err(), "cursor published an event");
    }

    #[test]
    fn session_launch_carries_only_the_cursor_owned_by_its_provider() {
        let mut thread = thread();
        let cursor = crate::provider::ResumeCursor {
            provider: thread.provider.clone(),
            value: json!({"version": 3, "opaque": true}),
        };
        thread.provider_resume_cursor = Some(cursor.clone());
        let prepared = PreparedDriver {
            identity: thread.provider.clone(),
            driver: DriverStart::Claude(crate::config::ClaudeSettings {
                enabled: true,
                binary_path: "claude".to_string(),
                home_path: String::new(),
                launch_args: String::new(),
                custom_models: Vec::new(),
            }),
        };
        let start = starting(&thread, "/work", prepared);
        assert_eq!(start.resume_cursor, Some(cursor));
    }
}
