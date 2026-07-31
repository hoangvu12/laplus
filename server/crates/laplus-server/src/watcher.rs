//! Hearing about changes the app did not make.
//!
//! [`crate::filesystem::Index`] holds the last scan of each workspace so that a
//! keystroke in the `@` mention costs a lookup rather than a walk. Ticket 07
//! left that scan invalidated by exactly one thing — the app's own
//! `projects.writeFile` — which is honest only while the app is the only writer.
//! It is not: the agent writes, `cargo` writes, the developer's editor writes.
//! This module is the other door in.
//!
//! ## One watcher, not one per workspace
//!
//! `notify`'s Windows backend spawns a thread per `RecommendedWatcher`, and
//! that thread services every path registered on it. So there is a single
//! instance here and workspaces are added to and removed from it, rather than
//! an instance per workspace — which makes "no threads leaked" a property of
//! the design rather than something to remember at each release. It is created
//! on the first [`Watcher::watch`], so a server nobody opens a project in
//! spawns nothing at all.
//!
//! ## What a large repository costs, and where that stops being true
//!
//! On Windows — v1's only platform — a recursive watch is a single
//! `ReadDirectoryChangesW` registration on the root directory, so a repository
//! of twenty-five thousand files costs exactly what an empty one does: one
//! handle, on the thread above. Nothing here walks, scans or polls, so there is
//! no per-file work at any point and nothing that could occupy a core.
//!
//! That is a property of the backend rather than of this code, and it does not
//! survive a port. `inotify` on Linux registers a watch **per directory**, so a
//! large repository there can meet `max_user_watches` (commonly 8192) and the
//! `watch` below will start refusing. The refusal is already handled the way it
//! should be — the workspace is logged and left unwatched, and everything about
//! it still works — but a Linux build would want subtree exclusions before it
//! could claim this section's first paragraph.
//!
//! ## What this module does not know
//!
//! It does not know what a listing is, which paths matter, or what to do about
//! one. It turns an operating-system event into "this workspace, this relative
//! path" and hands that to a callback. The rule about which paths are worth
//! acting on lives with the thing that can answer it — the held listing, in
//! [`crate::filesystem`] — because "is this path part of the workspace" is a
//! question only the last scan can answer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// How many workspaces may be watched at once.
///
/// Not an operating-system limit — on Windows each root is one directory handle
/// on one shared thread, and a session with more than a handful of projects
/// open does not exist. It is here because `projects.listEntries` takes the
/// workspace from the client, so without a ceiling the number of handles the
/// server holds is decided by whatever is on the other end of the socket.
///
/// Past the ceiling the **least recently listed** workspace is evicted, and it
/// is worth saying why that is the right end to drop from. Refusing the new
/// workspace instead — which is what this did first — hands the ceiling to
/// whoever fills it first: sixteen folders a client listed once and abandoned
/// would lock out the project the user is actually working in, and the only
/// sign of it would be a line on stderr. Evicting by least-recently-listed
/// makes the surviving set the projects that are being *used*, because
/// `listEntries` is the UI opening a project or pressing its refresh button.
///
/// An evicted workspace loses only freshness: its held scan goes back to being
/// invalidated by the app's own writes alone, which is the behaviour before
/// this ticket, and the next `listEntries` both rescans it and watches it
/// again.
pub const MAX_WATCHED: usize = 16;

/// Told that something under a watched workspace changed.
///
/// The first argument is the key the workspace was watched under; the second is
/// the changed path relative to its root, with forward slashes and no leading
/// separator — the same spelling [`crate::filesystem`] gives its entries, so
/// the two can be compared without either side normalising again. An empty
/// string means the root itself.
type OnChange = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Every workspace being watched, and the one `notify` instance behind them.
///
/// Cheap to clone; every clone is the same watcher. [`crate::filesystem::Index`]
/// holds one and is itself cloned into each deferred call, so this has to be a
/// handle rather than a value.
#[derive(Clone)]
pub struct Watcher {
    /// `None` until the first workspace is watched. Behind its own lock so that
    /// creating it does not have to happen under the registry's.
    notify: Arc<Mutex<Option<RecommendedWatcher>>>,
    watched: Arc<Mutex<Vec<Watched>>>,
    on_change: OnChange,
}

/// One workspace on the shared watcher.
///
/// Ordered least-recently-listed first inside [`Watcher`], which is what makes
/// the eviction at [`MAX_WATCHED`] drop the workspace nobody is looking at.
struct Watched {
    /// [`crate::projects::WorkspaceRoot::canonical`] — the same key the index
    /// holds its scan under.
    key: String,
    root: PathBuf,
    /// The root as text, so an event's path can be tested against it without
    /// rebuilding a `Path` per event.
    ///
    /// The same value as `root`, twice, which is why [`Watched::new`] is the
    /// only way to build one — the two must not be able to disagree.
    root_text: String,
}

impl Watched {
    fn new(key: &str, root: &Path) -> Watched {
        Watched {
            key: key.to_string(),
            root_text: root.to_string_lossy().into_owned(),
            root: root.to_path_buf(),
        }
    }
}

impl Watcher {
    pub fn new(on_change: impl Fn(&str, &str) + Send + Sync + 'static) -> Watcher {
        Watcher {
            notify: Arc::new(Mutex::new(None)),
            watched: Arc::new(Mutex::new(Vec::new())),
            on_change: Arc::new(on_change),
        }
    }

    /// Watch `root` under `key`, recursively. Idempotent, and silent when it
    /// cannot: a workspace that will not watch still lists, searches and reads
    /// exactly as it did before, so a failure here is a loss of freshness
    /// rather than of function.
    ///
    /// **Takes the registry lock and never the index's.** The event handler
    /// takes them the other way round — registry, then whatever `on_change`
    /// touches — so a caller must not already be holding the index's scans when
    /// it arrives here. [`crate::filesystem::Index::rescan`] is written to that
    /// rule.
    /// **The registry lock is never held across a call into the backend.** See
    /// [`Watcher::release`] for what that costs on Linux: `inotify` waits for
    /// its event thread to acknowledge a registration, and that thread may be
    /// inside `deliver` holding — or waiting for — this lock. The three
    /// sections below are therefore three separate acquisitions rather than one.
    pub fn watch(&self, key: &str, root: &Path) {
        {
            let mut watched = lock(&self.watched);
            if let Some(position) = watched.iter().position(|entry| entry.key == key) {
                // Already watched, and just listed again — so it goes to the
                // back of the eviction order. This is the whole of how "recently
                // used" is recorded: `listEntries` is the only caller, and it is
                // the UI opening a project or refreshing it.
                let touched = watched.remove(position);
                watched.push(touched);
                return;
            }
        }

        {
            let mut notify = lock(&self.notify);
            if notify.is_none() {
                match self.start() {
                    Ok(started) => *notify = Some(started),
                    Err(error) => {
                        eprintln!("laplus: cannot watch the filesystem: {error}");
                        return;
                    }
                }
            }
            let started = notify.as_mut().expect("a watcher was just created");
            if let Err(error) = started.watch(root, RecursiveMode::Recursive) {
                eprintln!("laplus: cannot watch {}: {error}", root.display());
                return;
            }
        }

        // Evicted *after* the new root is registered, so a watch that fails does
        // not also cost the workspace it would have replaced.
        let evicted = {
            let mut watched = lock(&self.watched);
            // Re-checked, because the lock was let go above: a second call for
            // the same key could have registered it while the backend was being
            // told about this one. Registering a path twice is idempotent in
            // `notify`, so the loser of that race has nothing to undo — but two
            // entries under one key would leak a watch on the next release.
            if watched.iter().any(|entry| entry.key == key) {
                return;
            }
            let evicted = (watched.len() >= MAX_WATCHED).then(|| watched.remove(0));
            watched.push(Watched::new(key, root));
            evicted
        };

        if let Some(evicted) = evicted {
            if let Some(notify) = lock(&self.notify).as_mut() {
                let _ = notify.unwatch(&evicted.root);
            }
            eprintln!(
                "laplus: {MAX_WATCHED} workspaces are already watched, so changes \
                 under {} will only be noticed when it is listed again",
                evicted.root.display()
            );
        }
    }

    /// Stop watching whatever is held under `key`, releasing its handle.
    ///
    /// Returns whether there was one. What "a project is closed" means on this
    /// wire is `project.delete` — see [`crate::orchestration`] — and that is the
    /// caller.
    pub fn release(&self, key: &str) -> bool {
        // **The registry lock is dropped before the backend is touched**, and on
        // Linux that is the difference between working and wedging. `inotify`'s
        // `unwatch` waits for its own event thread to acknowledge the removal,
        // and that thread may be inside `deliver` waiting for this very lock —
        // so holding it across the call is a deadlock with the operating
        // system's timing as the trigger. It never fired on Windows, where
        // `ReadDirectoryChangesW` needs no such handshake, which is exactly why
        // it survived until the suite was first run on Linux.
        let gone = {
            let mut watched = lock(&self.watched);
            let Some(position) = watched.iter().position(|entry| entry.key == key) else {
                return false;
            };
            watched.remove(position)
        };

        if let Some(notify) = lock(&self.notify).as_mut() {
            // An unwatch that fails has already lost the registration it was
            // asked to drop — the path went away, or the handle did. There is
            // nothing left to do about it and nothing the user could act on.
            let _ = notify.unwatch(&gone.root);
        }
        true
    }

    /// How many workspaces are watched. The gauge that says a release actually
    /// released something.
    pub fn len(&self) -> usize {
        lock(&self.watched).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn start(&self) -> notify::Result<RecommendedWatcher> {
        let watched = Arc::clone(&self.watched);
        let on_change = Arc::clone(&self.on_change);
        notify::recommended_watcher(move |event| match event {
            Ok(event) => deliver(&watched, &on_change, event),
            // A dropped or undecodable event is a gap in what the server was
            // told, not a reason to stop listening. The cost is one stale scan
            // until the next change or the next listing.
            Err(error) => eprintln!("laplus: a filesystem event was lost: {error}"),
        })
    }
}

impl std::fmt::Debug for Watcher {
    /// The count rather than the paths: this is printed inside
    /// [`crate::rpc::Services`], which reaches every dispatch trace, and a list
    /// of the user's project directories is not something to scatter through a
    /// log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Watcher")
            .field("watched", &self.len())
            .finish()
    }
}

/// Turn one operating-system event into calls on the change callback.
///
/// The registry lock is taken to work out which workspace each path belongs to
/// and **released before the callback runs**. That is what keeps the lock
/// ordering one-way: the callback reaches into the index, and the index reaches
/// into [`Watcher::watch`], so holding both at once in either order would be
/// half of a deadlock.
fn deliver(watched: &Mutex<Vec<Watched>>, on_change: &OnChange, event: notify::Event) {
    // Reading a file changes nothing that can be listed. Every other kind can,
    // and is not worth distinguishing: `ReadDirectoryChangesW` reports a
    // creation as a create *and* a modify and cannot say whether a modify
    // touched data or metadata, so a rule finer than this one would be a guess
    // dressed as a filter.
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }

    let changed: Vec<(String, String)> = {
        let watched = lock(watched);
        let mut changed = Vec::new();

        if event.need_rescan() {
            // The backend is telling us it dropped events. Which paths they
            // named is exactly what has been lost, so every watched workspace
            // is reported as changed at its root — the one path no listing can
            // dismiss.
            for entry in watched.iter() {
                changed.push((entry.key.clone(), String::new()));
            }
        } else {
            for path in &event.paths {
                // **Every** workspace that contains the path, not the first.
                // Workspaces nest: a user may have both a repository and one
                // of its packages open as projects, and stopping at the first
                // match would leave the inner one permanently stale — its
                // events all attributed to the outer root, which is watching a
                // superset and does not care.
                for entry in watched.iter() {
                    if let Some(relative) = entry.relative(path) {
                        changed.push((entry.key.clone(), relative));
                    }
                }
            }
        }

        changed
    };

    for (key, relative) in changed {
        on_change(&key, &relative);
    }
}

impl Watched {
    /// This path as the workspace names it, or `None` if it is not in this
    /// workspace at all.
    ///
    /// The comparison folds ASCII case because Windows paths do, and a backend
    /// that reported `C:\Repo\src` for a root watched as `C:\repo` would
    /// otherwise drop every event. It folds *only* ASCII: that keeps the
    /// prefix's length in bytes fixed, so the remainder can be taken from the
    /// original string with its own casing intact — which matters, because that
    /// remainder is compared against a listing built from real directory names.
    fn relative(&self, path: &Path) -> Option<String> {
        let text = path.to_string_lossy();
        // `get` rather than a slice: a byte length taken from one string is not
        // guaranteed to be a character boundary in another, and indexing would
        // panic where this returns "not ours".
        let head = text.get(..self.root_text.len())?;
        if !head.eq_ignore_ascii_case(&self.root_text) {
            return None;
        }

        // A prefix match on the *text* is not a prefix match on the path:
        // `C:\repo-other` begins with `C:\repo`. The remainder has to start at
        // a component boundary, which means it is empty, it begins with a
        // separator, or the root supplied one of its own — `C:\` does.
        let rest = &text[self.root_text.len()..];
        if !(rest.is_empty()
            || rest.starts_with(['/', '\\'])
            || self.root_text.ends_with(['/', '\\']))
        {
            return None;
        }

        Some(rest.trim_start_matches(['/', '\\']).replace('\\', "/"))
    }
}

/// A poisoned lock means a previous holder panicked while holding it. Neither
/// of the things behind these locks is left half-written by that — a `Vec` and
/// an `Option` — and refusing to watch anything again would turn one panic into
/// a file tree that never updates for the rest of the session.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// How long a test will wait for the operating system to report a change.
    ///
    /// Generously long, and not a claim about latency: on Windows a change is
    /// usually reported in tens of milliseconds. It is the bound that turns
    /// "never arrives" into a failure with a message instead of a test that
    /// hangs a CI run.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// A watcher that posts every change it is told about, so a test can wait
    /// for one rather than sleep for a guess.
    fn recording() -> (Watcher, mpsc::Receiver<(String, String)>) {
        let (changes, received) = mpsc::channel();
        let watcher = Watcher::new(move |key: &str, relative: &str| {
            let _ = changes.send((key.to_string(), relative.to_string()));
        });
        (watcher, received)
    }

    /// Wait for a change matching `wanted`, ignoring the others.
    ///
    /// Ignoring is the point: a single `std::fs::write` produces a create and a
    /// modify on Windows, and a temporary directory's own machinery can produce
    /// more. What a test can honestly assert is that the change it made was
    /// reported, not that nothing else was.
    fn wait_for(
        received: &mpsc::Receiver<(String, String)>,
        wanted: impl Fn(&str, &str) -> bool,
    ) -> (String, String) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no matching change arrived within {PATIENCE:?}");
            match received.recv_timeout(left) {
                Ok((key, relative)) if wanted(&key, &relative) => return (key, relative),
                Ok(_) => continue,
                Err(error) => panic!("no matching change arrived: {error}"),
            }
        }
    }

    /// The whole of what this module promises: a change made by something other
    /// than the server arrives as the workspace it happened in and the path it
    /// happened to.
    #[test]
    fn a_file_written_outside_the_server_is_reported_relative_to_its_workspace() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        // Linux adds recursive watches one directory at a time. Keep creating a
        // new subtree out of this path-spelling test; the directory event alone
        // is enough to invalidate the held listing.
        std::fs::create_dir(directory.path().join("src")).expect("creates the directory");

        let (watcher, received) = recording();
        watcher.watch("workspace", directory.path());

        std::fs::write(directory.path().join("src").join("main.rs"), "fn main() {}")
            .expect("writes the file");

        let (key, relative) = wait_for(&received, |_, relative| relative == "src/main.rs");
        assert_eq!(key, "workspace");
        assert_eq!(
            relative, "src/main.rs",
            "the path is not spelled the way a listing spells one"
        );
    }

    /// Releasing gives the handle back, and the workspace stops being reported.
    /// The count is the observable half; that nothing further arrives is the
    /// half that matters.
    #[test]
    fn a_released_workspace_is_no_longer_watched() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (watcher, received) = recording();

        watcher.watch("workspace", directory.path());
        assert_eq!(watcher.len(), 1);

        // Prove the watch was live before releasing it, so that the silence
        // afterwards means something.
        std::fs::write(directory.path().join("before.txt"), "before").expect("writes the file");
        wait_for(&received, |_, relative| relative == "before.txt");

        assert!(watcher.release("workspace"));
        assert!(watcher.is_empty());
        assert!(
            !watcher.release("workspace"),
            "releasing twice must not claim to have released twice"
        );

        while received.try_recv().is_ok() {}
        std::fs::write(directory.path().join("after.txt"), "after").expect("writes the file");
        // Long enough that the event would have arrived — the successful wait
        // above is the evidence for what "long enough" is on this machine.
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            received.try_recv().is_err(),
            "a released workspace is still being reported"
        );
    }

    /// Watching the same workspace twice must not register it twice: the
    /// index calls this on every listing, which is once per project open and
    /// once per press of the refresh button.
    #[test]
    fn watching_a_workspace_twice_registers_it_once() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (watcher, _received) = recording();

        watcher.watch("workspace", directory.path());
        watcher.watch("workspace", directory.path());

        assert_eq!(watcher.len(), 1);
    }

    /// Workspaces nest — a repository and one of its packages can both be open
    /// as projects — so a change inside the inner one belongs to *both*.
    ///
    /// Reporting only the outer root would leave the inner project permanently
    /// stale: the outer one is watching a superset and its own listing does not
    /// care about the change, so nothing would ever invalidate the inner scan.
    #[test]
    fn a_change_inside_a_nested_workspace_is_reported_to_both_of_them() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let inner = directory.path().join("packages").join("web");
        std::fs::create_dir_all(&inner).expect("creates the nested directory");

        let (watcher, received) = recording();
        watcher.watch("outer", directory.path());
        watcher.watch("inner", &inner);

        std::fs::write(inner.join("app.tsx"), "export default null")
            .expect("writes the file");

        // Each names the same file the way its own listing would.
        wait_for(&received, |key, relative| {
            key == "inner" && relative == "app.tsx"
        });
        wait_for(&received, |key, relative| {
            key == "outer" && relative == "packages/web/app.tsx"
        });
    }

    /// The ceiling is the answer to "the number of watched roots is decided by
    /// the client", and it drops the workspace nobody is looking at.
    ///
    /// The order matters more than the count: a client that lists sixteen
    /// folders once and abandons them must not be able to lock out the project
    /// the user is actually working in.
    #[test]
    fn past_the_ceiling_the_least_recently_listed_workspace_is_evicted() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (watcher, _received) = recording();
        let root = |name: &str| {
            let path = directory.path().join(name);
            std::fs::create_dir_all(&path).expect("creates the directory");
            path
        };

        let working_in = root("the-one-being-used");
        watcher.watch("the-one-being-used", &working_in);

        // Fill the rest of the ceiling with folders listed once and forgotten.
        for index in 0..MAX_WATCHED - 1 {
            watcher.watch(&format!("abandoned-{index}"), &root(&format!("abandoned-{index}")));
        }
        assert_eq!(watcher.len(), MAX_WATCHED);

        // Listing the real project again is what keeps it: it goes to the back
        // of the eviction order, so the next arrival displaces an abandoned one.
        watcher.watch("the-one-being-used", &working_in);
        for index in 0..4 {
            watcher.watch(&format!("later-{index}"), &root(&format!("later-{index}")));
        }

        assert_eq!(watcher.len(), MAX_WATCHED);
        assert!(
            watcher.release("the-one-being-used"),
            "the project being worked in was evicted in favour of abandoned folders"
        );
        assert!(
            !watcher.release("abandoned-0"),
            "the least recently listed workspace survived the ceiling"
        );
    }

    /// A workspace that cannot be watched — because it is not there — leaves
    /// the registry as it was rather than holding an entry for a handle that
    /// was never taken.
    #[test]
    fn a_workspace_that_cannot_be_watched_is_not_registered() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (watcher, _received) = recording();

        watcher.watch("gone", &directory.path().join("not-there"));

        assert!(watcher.is_empty());
    }

    /// The path arithmetic, without an operating system in the way. Case is
    /// folded because Windows folds it; the remainder keeps the casing it
    /// really has, because it is about to be compared against directory names
    /// that do.
    #[test]
    fn a_path_is_named_relative_to_its_workspace_whatever_case_it_arrives_in() {
        let watched = Watched {
            key: "workspace".to_string(),
            root: PathBuf::from(r"C:\repo"),
            root_text: r"C:\repo".to_string(),
        };

        assert_eq!(
            watched.relative(Path::new(r"C:\repo\src\Main.rs")),
            Some("src/Main.rs".to_string())
        );
        assert_eq!(
            watched.relative(Path::new(r"c:\REPO\src\main.rs")),
            Some("src/main.rs".to_string())
        );
        // The root itself, which no listing can dismiss.
        assert_eq!(watched.relative(Path::new(r"C:\repo")), Some(String::new()));

        // A sibling whose name merely starts the same way is not inside it.
        assert_eq!(watched.relative(Path::new(r"C:\repo-other\src")), None);
        assert_eq!(watched.relative(Path::new(r"C:\elsewhere\src")), None);
        assert_eq!(watched.relative(Path::new(r"C:")), None);
    }

    /// A root that already ends in a separator — a drive root is the one that
    /// really happens — must not leave the remainder with a leading one.
    #[test]
    fn a_root_that_ends_in_a_separator_does_not_double_it() {
        let watched = Watched {
            key: "drive".to_string(),
            root: PathBuf::from(r"C:\"),
            root_text: r"C:\".to_string(),
        };

        assert_eq!(
            watched.relative(Path::new(r"C:\repo\src")),
            Some("repo/src".to_string())
        );
    }
}
