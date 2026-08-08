//! Live invalidation of the library scan cache.
//!
//! Notify callbacks only enqueue events. One owned worker holds the watcher,
//! debounces bursts, rescans provider/root fragments and publishes an assembled
//! snapshot after the corresponding cache transaction commits. That ownership
//! is important: a manual rebuild and a filesystem-triggered rebuild can never
//! race to put an older snapshot back in the cache.

use crate::library::{self, LibraryPlan, LibraryScope, LibrarySnapshot, ScopeSnapshots};
use crate::providers::{self, Runner};
use crate::store;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const LIBRARY_UPDATED_EVENT: &str = "aviary://library-updated";
const SCOPE_CACHE_PREFIX: &str = "library:scope:";
const DEBOUNCE: Duration = Duration::from_millis(300);
// A tool that continuously rewrites config should not postpone refresh forever.
const MAX_DEBOUNCE: Duration = Duration::from_millis(1_500);
// Native backends can emit thousands of events during a plugin install. Once
// this fills, one overflow bit replaces further detail and forces a full scan.
const EVENT_QUEUE_CAPACITY: usize = 256;
// std channels cannot select across control and filesystem receivers. Polling
// the event queue for this long keeps control traffic responsive without a
// wake message per native event.
const CONTROL_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Serialize)]
pub struct LibraryUpdated {
    pub revision: u64,
    pub changed_paths: Vec<String>,
    pub scopes: Vec<String>,
    pub snapshot: LibrarySnapshot,
}

enum Control {
    Rebuild(mpsc::SyncSender<Result<LibrarySnapshot, String>>),
    ProjectsChanged(mpsc::SyncSender<Result<LibrarySnapshot, String>>),
    Shutdown,
}

/// Cloneable handle managed by Tauri; the worker and native watcher have one
/// owner and are shut down when the last handle goes away.
#[derive(Clone)]
pub struct LibraryWatcher {
    inner: Arc<WatcherHandle>,
}

struct WatcherHandle {
    control_tx: Mutex<Option<mpsc::Sender<Control>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl WatcherHandle {
    fn send(&self, control: Control) -> Result<(), String> {
        let guard = self.control_tx.lock().unwrap_or_else(|e| e.into_inner());
        let tx = guard
            .as_ref()
            .ok_or_else(|| "library watcher stopped".to_string())?;
        tx.send(control)
            .map_err(|_| "library watcher stopped".to_string())
    }

    fn shutdown(&self) {
        // Taking the sender under the same mutex used by `send` makes the
        // close boundary exact: controls accepted before it are ahead of
        // Shutdown in FIFO order; controls after it fail instead of hanging.
        let tx = self
            .control_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(tx) = tx {
            let _ = tx.send(Control::Shutdown);
        }
        if let Some(join) = self.join.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = join.join();
        }
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl LibraryWatcher {
    /// Starts native watches immediately, then performs the first full scan on
    /// the worker. The existing aggregate cache remains available while that
    /// startup revalidation is running.
    pub fn start(app: AppHandle) -> Result<Self, String> {
        let (control_tx, control_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let worker_overflowed = Arc::clone(&overflowed);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("aviary-library-watcher".into())
            .spawn(move || {
                let mut worker = Worker::new(app, event_tx, worker_overflowed);
                let _ = ready_tx.send(());
                if let Err(error) = worker.refresh_all(BTreeSet::new()) {
                    log::error!("initial library revalidation failed: {error}");
                }
                worker.run(control_rx, event_rx);
                let _ = done_tx.send(());
            })
            .map_err(|e| format!("could not start library watcher: {e}"))?;

        if let Err(error) = ready_rx.recv_timeout(Duration::from_secs(5)) {
            let _ = control_tx.send(Control::Shutdown);
            // A platform watcher constructor that is itself wedged must not
            // turn the startup timeout into an unbounded wait. If the worker
            // acknowledges promptly, still join it to release every resource.
            match done_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = join.join();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            return Err(format!("library watcher did not initialise: {error}"));
        }

        Ok(Self {
            inner: Arc::new(WatcherHandle {
                control_tx: Mutex::new(Some(control_tx)),
                join: Mutex::new(Some(join)),
            }),
        })
    }

    /// Breaks the worker/AppHandle/managed-state ownership cycle. Safe to call
    /// more than once, including again from the final `Drop`.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// Serves the last real snapshot immediately unless a fresh walk was
    /// requested. Call this from `spawn_blocking`, never from a sync command.
    pub fn snapshot(&self, fresh: bool) -> Result<LibrarySnapshot, String> {
        if !fresh {
            if let Some(hit) = store::read_scan("library") {
                if let Ok(snapshot) = serde_json::from_str(&hit.payload) {
                    return Ok(snapshot);
                }
            }
        }
        self.request_rebuild(Control::Rebuild)
    }

    /// Reconciles registered-project watches and publishes the resulting index.
    /// The project database mutation must have committed before this is called.
    pub fn projects_changed(&self) -> Result<LibrarySnapshot, String> {
        self.request_rebuild(Control::ProjectsChanged)
    }

    fn request_rebuild(
        &self,
        make: fn(mpsc::SyncSender<Result<LibrarySnapshot, String>>) -> Control,
    ) -> Result<LibrarySnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.inner.send(make(reply_tx))?;
        reply_rx
            .recv()
            .map_err(|_| "library watcher stopped before replying".to_string())?
    }
}

struct Worker {
    app: AppHandle,
    watcher: Option<RecommendedWatcher>,
    watched: BTreeMap<PathBuf, bool>,
    plan: LibraryPlan,
    fragments: ScopeSnapshots,
    revision: u64,
    pending: Pending,
    overflowed: Arc<AtomicBool>,
}

impl Worker {
    fn new(
        app: AppHandle,
        event_tx: mpsc::SyncSender<notify::Result<Event>>,
        overflowed: Arc<AtomicBool>,
    ) -> Self {
        let notify_tx = event_tx.clone();
        let notify_overflowed = Arc::clone(&overflowed);
        let watcher = match notify::recommended_watcher(move |event| {
            match notify_tx.try_send(event) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    // Detail was lost, so a targeted refresh is no longer
                    // trustworthy. The worker converts this bit into a full
                    // refresh before consuming the next queued event.
                    notify_overflowed.store(true, Ordering::Release);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {}
            }
        }) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                // Manual rebuilds remain functional when a volume cannot be
                // observed, which is preferable to making Aviary unlaunchable.
                log::error!("native file watching is unavailable: {error}");
                None
            }
        };
        let mut worker = Self {
            app,
            watcher,
            watched: BTreeMap::new(),
            plan: LibraryPlan::current(),
            fragments: BTreeMap::new(),
            revision: 0,
            pending: Pending::default(),
            overflowed,
        };
        worker.reconcile_watches();
        worker
    }

    fn run(
        &mut self,
        control_rx: mpsc::Receiver<Control>,
        event_rx: mpsc::Receiver<notify::Result<Event>>,
    ) {
        loop {
            // Controls have their own channel and are checked before every
            // filesystem event. A saturated native-event queue therefore
            // cannot starve a rebuild request or application shutdown.
            match control_rx.try_recv() {
                Ok(control) => {
                    if !self.handle_control(control) {
                        break;
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if self.overflowed.swap(false, Ordering::AcqRel) {
                self.pending.mark_all();
            }
            // Check before receiving: at a zero timeout, a continuously
            // nonempty queue would otherwise return `Ok` forever and starve
            // both the bounded refresh deadline and control messages.
            if self.pending.is_due(Instant::now()) {
                self.flush_pending();
                continue;
            }
            let wait = self
                .pending
                .deadline()
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(CONTROL_POLL)
                .min(CONTROL_POLL);
            match event_rx.recv_timeout(wait) {
                Ok(event) => self.handle_event(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn handle_control(&mut self, control: Control) -> bool {
        match control {
            Control::Rebuild(reply) => {
                self.pending.clear();
                self.overflowed.store(false, Ordering::Release);
                self.reconcile_watches();
                let _ = reply.send(self.refresh_all(BTreeSet::new()));
            }
            Control::ProjectsChanged(reply) => {
                self.pending.clear();
                self.overflowed.store(false, Ordering::Release);
                self.plan = LibraryPlan::current();
                self.reconcile_watches();
                let _ = reply.send(self.refresh_all(BTreeSet::new()));
            }
            Control::Shutdown => return false,
        }
        true
    }

    fn handle_event(&mut self, event: notify::Result<Event>) {
        match event {
            Ok(event) if event.need_rescan() => self.pending.mark_all(),
            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                for path in event.paths {
                    let scopes = affected_scopes(&self.plan, &self.fragments, &path);
                    self.pending.add(path, scopes);
                }
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!("file watcher reported an error; scheduling full scan: {error}");
                self.pending.mark_all();
            }
        }
    }

    fn flush_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        self.reconcile_watches();
        let result = if pending.all {
            self.refresh_all(pending.paths).map(|_| ())
        } else if pending.scopes.is_empty() {
            Ok(())
        } else {
            self.refresh_scopes(&pending.scopes, pending.paths)
                .map(|_| ())
        };
        if let Err(error) = result {
            log::error!("live library refresh failed: {error}");
        }
        // A relevant directory may have been created during the burst. The
        // rescan sees its contents; this second pass makes future changes live.
        self.reconcile_watches();
    }

    fn refresh_all(&mut self, changed_paths: BTreeSet<PathBuf>) -> Result<LibrarySnapshot, String> {
        let started = Instant::now();
        let mut fragments = BTreeMap::new();
        for scope in self.plan.scopes() {
            fragments.insert(scope.clone(), library::scan_scope(scope));
        }
        self.fragments = fragments;
        let snapshot = library::assemble(
            &self.plan,
            &self.fragments,
            started.elapsed().as_millis() as u64,
        );
        let scopes: Vec<LibraryScope> = self.plan.scopes().to_vec();
        self.commit(snapshot, &scopes, changed_paths, true)
    }

    fn refresh_scopes(
        &mut self,
        requested: &BTreeSet<LibraryScope>,
        changed_paths: BTreeSet<PathBuf>,
    ) -> Result<LibrarySnapshot, String> {
        let started = Instant::now();
        let active: BTreeSet<LibraryScope> = self.plan.scopes().iter().cloned().collect();
        let mut refreshed = Vec::new();
        for scope in requested.intersection(&active) {
            self.fragments
                .insert(scope.clone(), library::scan_scope(scope));
            refreshed.push(scope.clone());
        }
        // This normally only happens if the aggregate cache was missing while
        // startup revalidation was queued. Fill gaps instead of assembling a
        // snapshot that silently omits a root.
        for scope in self.plan.scopes() {
            if !self.fragments.contains_key(scope) {
                self.fragments
                    .insert(scope.clone(), library::scan_scope(scope));
                refreshed.push(scope.clone());
            }
        }
        let snapshot = library::assemble(
            &self.plan,
            &self.fragments,
            started.elapsed().as_millis() as u64,
        );
        self.commit(snapshot.clone(), &refreshed, changed_paths, false)?;
        Ok(snapshot)
    }

    fn commit(
        &mut self,
        snapshot: LibrarySnapshot,
        refreshed: &[LibraryScope],
        changed_paths: BTreeSet<PathBuf>,
        replace_scopes: bool,
    ) -> Result<LibrarySnapshot, String> {
        let mut rows = Vec::with_capacity(refreshed.len() + 1);
        for scope in refreshed {
            let Some(fragment) = self.fragments.get(scope) else {
                continue;
            };
            rows.push((
                scope.cache_key(),
                serde_json::to_string(fragment).map_err(|e| e.to_string())?,
                fragment.scanned_ms,
            ));
        }
        rows.push((
            "library".to_string(),
            serde_json::to_string(&snapshot).map_err(|e| e.to_string())?,
            snapshot.scanned_ms,
        ));
        store::write_scan_batch(&rows, replace_scopes.then_some(SCOPE_CACHE_PREFIX))?;

        self.revision = self.revision.wrapping_add(1);
        let payload = LibraryUpdated {
            revision: self.revision,
            changed_paths: changed_paths
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            scopes: refreshed.iter().map(LibraryScope::cache_key).collect(),
            snapshot: snapshot.clone(),
        };
        if let Err(error) = self.app.emit(LIBRARY_UPDATED_EVENT, payload) {
            // The durable cache is already current; a later view load still
            // sees the right state if no webview was listening at startup.
            log::warn!("could not emit live library update: {error}");
        }
        Ok(snapshot)
    }

    fn reconcile_watches(&mut self) {
        let desired = desired_watches(&self.plan);
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };

        let removed: Vec<PathBuf> = self
            .watched
            .iter()
            .filter(|(path, recursive)| desired.get(*path) != Some(*recursive))
            .map(|(path, _)| path.clone())
            .collect();
        for path in removed {
            if let Err(error) = watcher.unwatch(&path) {
                log::warn!("could not stop watching {}: {error}", path.display());
            }
            self.watched.remove(&path);
        }

        for (path, recursive) in desired {
            if self.watched.get(&path) == Some(&recursive) {
                continue;
            }
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            match watcher.watch(&path, mode) {
                Ok(()) => {
                    self.watched.insert(path, recursive);
                }
                Err(error) => {
                    log::warn!("could not watch {}: {error}", path.display());
                }
            }
        }
    }
}

#[derive(Default)]
struct Pending {
    all: bool,
    scopes: BTreeSet<LibraryScope>,
    paths: BTreeSet<PathBuf>,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Pending {
    fn add(&mut self, path: PathBuf, scopes: BTreeSet<LibraryScope>) {
        if scopes.is_empty() {
            return;
        }
        self.scopes.extend(scopes);
        self.paths.insert(path);
        self.touch();
    }

    fn mark_all(&mut self) {
        self.all = true;
        self.touch();
    }

    fn touch(&mut self) {
        let now = Instant::now();
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    fn deadline(&self) -> Option<Instant> {
        Some(std::cmp::min(
            self.last? + DEBOUNCE,
            self.first? + MAX_DEBOUNCE,
        ))
    }

    fn is_due(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

fn affected_scopes(
    plan: &LibraryPlan,
    fragments: &ScopeSnapshots,
    path: &Path,
) -> BTreeSet<LibraryScope> {
    let mut affected = BTreeSet::new();
    let event_path = comparable_path(path);
    for scope in plan.scopes() {
        let hit = match scope {
            LibraryScope::User {
                runner: Runner::ClaudeCode,
                root,
            } => providers::claude_code::affects_user(&comparable_path(root), &event_path),
            LibraryScope::User {
                runner: Runner::Codex,
                root,
            } => providers::codex::affects_user(&comparable_path(root), &event_path),
            LibraryScope::Project {
                runner: Runner::ClaudeCode,
                path: project,
                ..
            } => providers::claude_code::affects_project(&comparable_path(project), &event_path),
            LibraryScope::Project {
                runner: Runner::Codex,
                path: project,
                ..
            } => providers::codex::affects_project(&comparable_path(project), &event_path),
        };
        if hit {
            affected.insert(scope.clone());
        }
    }

    let Some(home) = plan.home() else {
        return affected;
    };
    let agents = comparable_path(&home.join(".agents"));
    let shared = agents.join("skills");
    if event_path == agents || event_path == shared {
        // Creation/removal can repair or break any previously unresolved link.
        affected.extend(plan.scopes().iter().cloned());
    } else {
        if !event_path.starts_with(&shared) {
            return affected;
        }
        // A shared skill is only library-visible through links from a provider
        // root. Map the canonical target back to precisely those fragments. An
        // edit reported through one runner's symlink is canonicalised first so
        // every runner that points at the same file gets a fresh fragment.
        let mut mapped = false;
        for (scope, fragment) in fragments {
            if fragment.entries.iter().any(|entry| {
                let real = comparable_path(Path::new(&entry.real_path));
                real == event_path || real.starts_with(&event_path)
            }) {
                mapped = true;
                affected.insert(scope.clone());
            }
        }
        if !mapped {
            // Broken links have no entry/real_path in a fragment. Recreating
            // their shared target therefore cannot be mapped precisely; a
            // conservative full refresh is the only way to repair every link.
            affected.extend(plan.scopes().iter().cloned());
        }
    }
    affected
}

/// Canonicalises even a just-deleted path by resolving its nearest surviving
/// ancestor and reattaching the missing suffix. macOS spells temporary paths
/// both as `/var/...` and `/private/var/...`; comparing either raw spelling
/// would make real symlink targets intermittently miss their scope.
fn comparable_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    let mut ancestor = path;
    let mut suffix = Vec::new();
    while let Some(parent) = ancestor.parent() {
        if let Some(name) = ancestor.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(mut canonical) = std::fs::canonicalize(parent) {
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }
        ancestor = parent;
    }
    path.to_path_buf()
}

/// Desired native watches. `bool` is recursive; duplicate paths are upgraded
/// to recursive rather than registered twice.
fn desired_watches(plan: &LibraryPlan) -> BTreeMap<PathBuf, bool> {
    let mut out = BTreeMap::new();
    let mut add = |path: PathBuf, recursive: bool| {
        if !path.exists() {
            return;
        }
        out.entry(path)
            .and_modify(|current| *current |= recursive)
            .or_insert(recursive);
    };

    if let Some(home) = plan.home() {
        add(home.to_path_buf(), false);
        let agents = home.join(".agents");
        add(agents.clone(), false);
        add(agents.join("skills"), true);
    }

    for scope in plan.scopes() {
        match scope {
            LibraryScope::User {
                runner: Runner::ClaudeCode,
                root,
            } => {
                add(root.clone(), false);
                for path in providers::claude_code::user_recursive_roots(root) {
                    add(path, true);
                }
            }
            LibraryScope::User {
                runner: Runner::Codex,
                root,
            } => {
                add(root.clone(), false);
                for path in providers::codex::user_recursive_roots(root) {
                    add(path, true);
                }
            }
            LibraryScope::Project {
                runner: Runner::ClaudeCode,
                path,
                ..
            } => {
                if let Some(parent) = path.parent() {
                    add(parent.to_path_buf(), false);
                }
                add(path.clone(), false);
                add(path.join(".claude"), false);
                for root in providers::claude_code::project_recursive_roots(path) {
                    add(root, true);
                }
            }
            LibraryScope::Project {
                runner: Runner::Codex,
                path,
                ..
            } => {
                if let Some(parent) = path.parent() {
                    add(parent.to_path_buf(), false);
                }
                add(path.clone(), false);
                add(path.join(".codex"), false);
                for root in providers::codex::project_recursive_roots(path) {
                    add(root, true);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_home(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aviary-watcher-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn shared_target_maps_back_to_every_linking_scope() {
        use std::os::unix::fs::symlink;

        let home = temp_home("shared-scopes");
        let real = home.join(".agents/skills/demo");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("SKILL.md"),
            "---\nname: Demo\ndescription: before\n---\nbody\n",
        )
        .unwrap();
        for root in [home.join(".claude/skills"), home.join(".codex/skills")] {
            fs::create_dir_all(&root).unwrap();
            symlink(&real, root.join("demo")).unwrap();
        }

        let plan = LibraryPlan::for_home(home.clone(), vec![]);
        let (_, fragments) = library::scan_plan(&plan);
        let affected = affected_scopes(&plan, &fragments, &real.join("SKILL.md"));
        assert_eq!(affected.len(), 2);
        assert!(affected
            .iter()
            .any(|scope| scope.runner() == Runner::ClaudeCode));
        assert!(affected.iter().any(|scope| scope.runner() == Runner::Codex));

        let through_claude = affected_scopes(
            &plan,
            &fragments,
            &home.join(".claude/skills/demo/SKILL.md"),
        );
        assert_eq!(
            through_claude.len(),
            2,
            "a symlink-path event must refresh every root sharing its target"
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recreating_a_broken_shared_target_refreshes_every_scope() {
        use std::os::unix::fs::symlink;

        let home = temp_home("repair-shared-scopes");
        let real = home.join(".agents/skills/repaired");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        for root in [home.join(".claude/skills"), home.join(".codex/skills")] {
            fs::create_dir_all(&root).unwrap();
            symlink(&real, root.join("repaired")).unwrap();
        }

        let plan = LibraryPlan::for_home(home.clone(), vec![]);
        let (_, fragments) = library::scan_plan(&plan);
        assert!(fragments
            .values()
            .all(|fragment| fragment.entries.is_empty()));

        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("SKILL.md"),
            "---\nname: Repaired\ndescription: live again\n---\nbody\n",
        )
        .unwrap();
        let affected = affected_scopes(&plan, &fragments, &real.join("SKILL.md"));
        assert_eq!(
            affected.len(),
            plan.scopes().len(),
            "an unmapped recreated target must repair every possible link"
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_watcher_observes_atomic_replace_in_a_real_directory() {
        let root = temp_home("atomic-replace");
        let skill = root.join("demo");
        fs::create_dir_all(&skill).unwrap();
        let target = skill.join("SKILL.md");
        fs::write(&target, "before").unwrap();

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx).unwrap();
        watcher.watch(&root, RecursiveMode::Recursive).unwrap();
        // FSEvents installs its stream asynchronously after `watch` returns.
        // Give registration time to settle before exercising the rename shape.
        std::thread::sleep(Duration::from_millis(250));
        let temporary = skill.join(".editor-save");
        fs::write(&temporary, "after").unwrap();
        fs::rename(&temporary, &target).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let expected = comparable_path(&target);
        let mut saw_target = false;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(Ok(event))
                    if event
                        .paths
                        .iter()
                        .any(|path| comparable_path(path) == expected) =>
                {
                    saw_target = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        drop(watcher);
        assert!(
            saw_target,
            "atomic replacement must report the destination path"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn maximum_deadline_wins_during_an_event_storm() {
        let now = Instant::now();
        let pending = Pending {
            all: false,
            scopes: BTreeSet::new(),
            paths: BTreeSet::new(),
            first: Some(now - MAX_DEBOUNCE),
            // A new event just arrived, so the trailing debounce alone would
            // still be in the future.
            last: Some(now),
        };
        assert!(pending.is_due(now));
    }

    #[test]
    fn control_traffic_is_independent_of_a_saturated_event_queue() {
        let (event_tx, _event_rx) = mpsc::sync_channel::<notify::Result<Event>>(1);
        event_tx.send(Ok(Event::new(EventKind::Any))).unwrap();
        assert!(matches!(
            event_tx.try_send(Ok(Event::new(EventKind::Any))),
            Err(mpsc::TrySendError::Full(_))
        ));

        let (control_tx, control_rx) = mpsc::channel();
        control_tx.send(Control::Shutdown).unwrap();
        assert!(matches!(control_rx.try_recv(), Ok(Control::Shutdown)));
    }

    #[test]
    fn watcher_handle_shutdown_is_idempotent() {
        let (control_tx, control_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let join = std::thread::spawn(move || {
            assert!(matches!(control_rx.recv(), Ok(Control::Shutdown)));
            worker_stopped.store(true, Ordering::Release);
        });
        let handle = WatcherHandle {
            control_tx: Mutex::new(Some(control_tx)),
            join: Mutex::new(Some(join)),
        };

        handle.shutdown();
        handle.shutdown();
        assert!(stopped.load(Ordering::Acquire));
    }
}
