//! Opt-in OS file watching for DuckDB source roots.
//!
//! `RAG_WATCH_ROOTS` (`;`-separated directories) enables an OS file watcher
//! built on `notify`. Events are debounced (`RAG_WATCH_DEBOUNCE_MS`, default
//! **2000**) and a minimum interval between syncs
//! (`RAG_WATCH_MIN_SYNC_INTERVAL_SECS`, default **30**) guards against watcher
//! storms: bursts inside the cooldown keep accumulating instead of scheduling
//! back-to-back syncs. Each flush runs the existing manifest-aware
//! `sync_sources` scan per affected root and records an ops-log entry
//! (`op = "watch_sync"`). Disabled by default.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::event::EventKind;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::AppError;

/// Default quiet period before a burst of events triggers a sync.
pub const DEFAULT_DEBOUNCE_MS: u64 = 2_000;
/// Default minimum interval between watcher-triggered syncs (storm guard).
pub const DEFAULT_MIN_SYNC_INTERVAL_SECS: u64 = 30;

/// Watcher configuration parsed from the environment.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Canonicalized directories watched recursively.
    pub roots: Vec<PathBuf>,
    /// Quiet period after the last event before pending roots sync.
    pub debounce: Duration,
    /// Minimum interval between sync flushes.
    pub min_sync_interval: Duration,
}

/// Read [`WatchConfig`] from `RAG_WATCH_ROOTS` / `RAG_WATCH_DEBOUNCE_MS` /
/// `RAG_WATCH_MIN_SYNC_INTERVAL_SECS`. `Ok(None)` means watching is disabled.
pub fn config_from_env() -> Result<Option<WatchConfig>, AppError> {
    config_from_values(
        std::env::var("RAG_WATCH_ROOTS").ok().as_deref(),
        std::env::var("RAG_WATCH_DEBOUNCE_MS").ok().as_deref(),
        std::env::var("RAG_WATCH_MIN_SYNC_INTERVAL_SECS")
            .ok()
            .as_deref(),
    )
}

/// Pure parser behind [`config_from_env`] (unit-testable, no env mutation).
///
/// Roots are canonicalized and must exist as directories. Nested duplicates
/// are dropped: a watched parent already covers its children.
pub fn config_from_values(
    roots: Option<&str>,
    debounce_ms: Option<&str>,
    min_sync_interval_secs: Option<&str>,
) -> Result<Option<WatchConfig>, AppError> {
    let Some(raw_roots) = roots else {
        return Ok(None);
    };
    let mut canonical: Vec<PathBuf> = Vec::new();
    for piece in raw_roots
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let path = PathBuf::from(piece);
        let canonicalized = std::fs::canonicalize(&path).map_err(|error| {
            AppError::config(format!(
                "RAG_WATCH_ROOTS entry '{piece}' is not readable: {error}"
            ))
        })?;
        if !canonicalized.is_dir() {
            return Err(AppError::config(format!(
                "RAG_WATCH_ROOTS entry '{piece}' is not a directory"
            )));
        }
        if !canonical.contains(&canonicalized) {
            canonical.push(canonicalized);
        }
    }
    if canonical.is_empty() {
        return Ok(None);
    }
    // Drop roots nested inside another watched root (parent covers them).
    let snapshot = canonical.clone();
    canonical.retain(|root| {
        !snapshot
            .iter()
            .any(|other| other != root && root.starts_with(other))
    });
    let debounce =
        match debounce_ms {
            Some(raw) => Duration::from_millis(raw.parse::<u64>().map_err(|_| {
                AppError::config("RAG_WATCH_DEBOUNCE_MS must be a positive integer")
            })?),
            None => Duration::from_millis(DEFAULT_DEBOUNCE_MS),
        };
    let min_sync_interval = match min_sync_interval_secs {
        Some(raw) => Duration::from_secs(raw.parse::<u64>().map_err(|_| {
            AppError::config("RAG_WATCH_MIN_SYNC_INTERVAL_SECS must be a positive integer")
        })?),
        None => Duration::from_secs(DEFAULT_MIN_SYNC_INTERVAL_SECS),
    };
    if debounce.is_zero() || min_sync_interval.is_zero() {
        return Err(AppError::config(
            "RAG_WATCH_DEBOUNCE_MS and RAG_WATCH_MIN_SYNC_INTERVAL_SECS must be non-zero",
        ));
    }
    Ok(Some(WatchConfig {
        roots: canonical,
        debounce,
        min_sync_interval,
    }))
}

/// Debounce + storm-guard state machine. No I/O; fully unit-testable.
#[derive(Debug)]
pub struct SyncDebouncer {
    debounce: Duration,
    min_sync_interval: Duration,
    pending: BTreeSet<PathBuf>,
    last_event_at: Option<Instant>,
    last_flush_at: Option<Instant>,
}

impl SyncDebouncer {
    pub fn new(debounce: Duration, min_sync_interval: Duration) -> Result<Self, AppError> {
        if debounce.is_zero() || min_sync_interval.is_zero() {
            return Err(AppError::config("watcher intervals must be non-zero"));
        }
        Ok(Self {
            debounce,
            min_sync_interval,
            pending: BTreeSet::new(),
            last_event_at: None,
            last_flush_at: None,
        })
    }

    /// Record a change affecting `root` at `now`.
    pub fn record(&mut self, root: PathBuf, now: Instant) {
        self.pending.insert(root);
        self.last_event_at = Some(now);
    }

    /// True when pending roots exist, the event stream has been quiet for at
    /// least the debounce window, and the storm-guard cooldown since the last
    /// flush has elapsed.
    pub fn due(&self, now: Instant) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let quiet = self
            .last_event_at
            .is_some_and(|t| now.saturating_duration_since(t) >= self.debounce);
        if !quiet {
            return false;
        }
        match self.last_flush_at {
            Some(t) => now.saturating_duration_since(t) >= self.min_sync_interval,
            None => true,
        }
    }

    /// Take the pending roots, marking `now` as the last flush time.
    pub fn take(&mut self, now: Instant) -> Vec<PathBuf> {
        self.last_flush_at = Some(now);
        std::mem::take(&mut self.pending).into_iter().collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

/// One debounced event, normalized away from `notify` specifics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// A path under one of the watched roots changed.
    Changed(PathBuf),
    /// Something ambiguous happened (overflow, error, empty paths): rescan
    /// every watched root.
    Rescan,
}

/// Injectable event stream so the watch loop can be tested without a real
/// filesystem watcher.
pub trait WatchEventSource {
    /// Wait up to `timeout` for the next event. `None` on timeout or a dead
    /// stream.
    fn next_event(&mut self, timeout: Duration) -> Option<WatchEvent>;
}

/// Live event source backed by `notify`. Owns the watcher so registrations
/// stay alive.
pub struct NotifyEventSource {
    _watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<Result<notify::Event, notify::Error>>,
}

impl NotifyEventSource {
    pub fn new(roots: &[PathBuf]) -> Result<Self, AppError> {
        let (send, receiver) = mpsc::channel();
        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
            let _ = send.send(event);
        })
        .map_err(watch_error)?;
        for root in roots {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(watch_error)?;
        }
        Ok(Self {
            _watcher: watcher,
            receiver,
        })
    }
}

impl WatchEventSource for NotifyEventSource {
    fn next_event(&mut self, timeout: Duration) -> Option<WatchEvent> {
        match self.receiver.recv_timeout(timeout) {
            Ok(Ok(event)) => classify_event(&event),
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "file watcher event error; rescanning roots");
                Some(WatchEvent::Rescan)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::error!("file watcher event channel disconnected; stopping watch loop");
                None
            }
        }
    }
}

/// Normalize a `notify` event: ignore pure reads and noise paths, treat
/// ambiguous shapes as a full rescan.
fn classify_event(event: &notify::Event) -> Option<WatchEvent> {
    if matches!(event.kind, EventKind::Access(_)) {
        return None;
    }
    if matches!(event.kind, EventKind::Any | EventKind::Other) || event.paths.is_empty() {
        return Some(WatchEvent::Rescan);
    }
    // For renames, keep the destination (last path); the sync is a full
    // manifest scan of the root either way.
    event
        .paths
        .iter()
        .filter(|path| !is_noise_path(path))
        .last()
        .cloned()
        .map(WatchEvent::Changed)
}

/// Skip editor temp files and dependency/build directories that would
/// otherwise keep the debouncer hot without changing the corpus.
pub fn is_noise_path(path: &Path) -> bool {
    if path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | "target" | "node_modules")
        )
    }) {
        return true;
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy()) else {
        return true;
    };
    name.starts_with('.')
        || name.starts_with('#')
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name.ends_with(".tmp")
}

/// Find the most specific watched root containing `path`.
pub fn resolve_root<'a>(roots: &'a [PathBuf], path: &Path) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

/// Outcome counters for one watch-loop session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchLoopStats {
    pub events: u64,
    pub sync_runs: u64,
    pub sync_failures: u64,
}

/// Blocking watch loop with injected event source and sync callback.
///
/// `stop` is polled every `poll` interval; production passes `|| false`.
pub fn run_watch_loop(
    source: &mut dyn WatchEventSource,
    roots: &[PathBuf],
    debouncer: &mut SyncDebouncer,
    mut sync: impl FnMut(&Path) -> Result<(), String>,
    mut stop: impl FnMut() -> bool,
    poll: Duration,
) -> WatchLoopStats {
    let mut stats = WatchLoopStats::default();
    while !stop() {
        match source.next_event(poll) {
            Some(WatchEvent::Changed(path)) => {
                stats.events += 1;
                let now = Instant::now();
                match resolve_root(roots, &path) {
                    Some(root) => debouncer.record(root.clone(), now),
                    // Defensive: notify only reports watched paths, so an
                    // unmatched path means root layout changed; rescan all.
                    None => {
                        for root in roots {
                            debouncer.record(root.clone(), now);
                        }
                    }
                }
            }
            Some(WatchEvent::Rescan) => {
                stats.events += 1;
                let now = Instant::now();
                for root in roots {
                    debouncer.record(root.clone(), now);
                }
            }
            None => {}
        }
        let now = Instant::now();
        if debouncer.due(now) {
            for root in debouncer.take(now) {
                stats.sync_runs += 1;
                if sync(&root).is_err() {
                    stats.sync_failures += 1;
                }
            }
        }
    }
    stats
}

fn watch_error(error: notify::Error) -> AppError {
    AppError::db(format!("file watcher error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn debouncer() -> SyncDebouncer {
        SyncDebouncer::new(Duration::from_millis(100), Duration::from_millis(500)).unwrap()
    }

    #[test]
    fn debouncer_waits_for_quiet_period() {
        let mut d = debouncer();
        let t0 = Instant::now();
        d.record(PathBuf::from("/a"), t0);
        assert!(!d.due(t0));
        assert!(!d.due(t0 + Duration::from_millis(99)));
        assert!(d.due(t0 + Duration::from_millis(100)));
        assert_eq!(
            d.take(t0 + Duration::from_millis(100)),
            vec![PathBuf::from("/a")]
        );
        assert_eq!(d.pending_count(), 0);
        assert!(!d.due(t0 + Duration::from_millis(200)));
    }

    #[test]
    fn debouncer_coalesces_bursts_per_root() {
        let mut d = debouncer();
        let t0 = Instant::now();
        for i in 0..10 {
            d.record(PathBuf::from("/a"), t0 + Duration::from_millis(i * 10));
        }
        // Still inside the quiet window measured from the last event.
        assert!(!d.due(t0 + Duration::from_millis(95)));
        assert!(d.due(t0 + Duration::from_millis(190)));
        assert_eq!(d.take(t0 + Duration::from_millis(190)).len(), 1);
    }

    #[test]
    fn debouncer_storm_guard_delays_second_flush() {
        let mut d = debouncer();
        let t0 = Instant::now();
        d.record(PathBuf::from("/a"), t0);
        assert!(d.due(t0 + Duration::from_millis(100)));
        d.take(t0 + Duration::from_millis(100));
        // New events arrive shortly after the flush: quiet window passes but
        // the cooldown since the last flush has not.
        d.record(PathBuf::from("/a"), t0 + Duration::from_millis(200));
        assert!(!d.due(t0 + Duration::from_millis(300)));
        assert!(!d.due(t0 + Duration::from_millis(599)));
        assert!(d.due(t0 + Duration::from_millis(600)));
    }

    #[test]
    fn debouncer_rejects_zero_intervals() {
        assert!(SyncDebouncer::new(Duration::ZERO, Duration::from_secs(1)).is_err());
        assert!(SyncDebouncer::new(Duration::from_secs(1), Duration::ZERO).is_err());
    }

    #[test]
    fn config_disabled_without_roots() {
        assert!(config_from_values(None, None, None).unwrap().is_none());
        assert!(config_from_values(Some(" ; ; "), None, None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn config_parses_roots_and_drops_nested() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let spec = format!("{};{};", parent.display(), child.display());
        let cfg = config_from_values(Some(&spec), Some("1500"), Some("20"))
            .unwrap()
            .unwrap();
        let parent_canonical = std::fs::canonicalize(&parent).unwrap();
        assert_eq!(cfg.roots, vec![parent_canonical]);
        assert_eq!(cfg.debounce, Duration::from_millis(1500));
        assert_eq!(cfg.min_sync_interval, Duration::from_secs(20));
    }

    #[test]
    fn config_rejects_missing_root_and_zero_intervals() {
        assert!(config_from_values(Some("/definitely/not/here"), None, None).is_err());
        let temp = tempfile::tempdir().unwrap();
        let spec = temp.path().display().to_string();
        assert!(config_from_values(Some(&spec), Some("0"), None).is_err());
        assert!(config_from_values(Some(&spec), None, Some("0")).is_err());
        assert!(config_from_values(Some(&spec), Some("abc"), None).is_err());
    }

    #[test]
    fn noise_paths_are_filtered() {
        assert!(is_noise_path(Path::new("/r/.git/index")));
        assert!(is_noise_path(Path::new("/r/target/debug/build")));
        assert!(is_noise_path(Path::new("/r/node_modules/x/index.js")));
        assert!(is_noise_path(Path::new("/r/.hidden")));
        assert!(is_noise_path(Path::new("/r/file.swp")));
        assert!(is_noise_path(Path::new("/r/#scratch#")));
        assert!(!is_noise_path(Path::new("/r/notes/today.md")));
        assert!(!is_noise_path(Path::new("/r/src/main.rs")));
    }

    #[test]
    fn resolve_root_prefers_longest_prefix() {
        let roots = vec![PathBuf::from("/a"), PathBuf::from("/a/b")];
        assert_eq!(
            resolve_root(&roots, Path::new("/a/b/c.md")),
            Some(&PathBuf::from("/a/b"))
        );
        assert_eq!(
            resolve_root(&roots, Path::new("/a/c.md")),
            Some(&PathBuf::from("/a"))
        );
        assert_eq!(resolve_root(&roots, Path::new("/elsewhere")), None);
    }

    /// Scripted event source for wiring tests. Once the script is empty it
    /// behaves like a real timeout: sleeps briefly and returns `None`, so
    /// debounce windows elapse in real time without a hot spin.
    struct FakeSource {
        script: VecDeque<WatchEvent>,
    }

    impl WatchEventSource for FakeSource {
        fn next_event(&mut self, timeout: Duration) -> Option<WatchEvent> {
            if self.script.is_empty() {
                std::thread::sleep(timeout);
                return None;
            }
            self.script.pop_front()
        }
    }

    fn run_fake(
        script: Vec<WatchEvent>,
        roots: &[PathBuf],
        debounce_ms: u64,
        min_interval_ms: u64,
        syncs: &mut Vec<PathBuf>,
        run_for_ms: u64,
    ) -> WatchLoopStats {
        let mut source = FakeSource {
            script: script.into(),
        };
        let mut debouncer = SyncDebouncer::new(
            Duration::from_millis(debounce_ms),
            Duration::from_millis(min_interval_ms),
        )
        .unwrap();
        let started = Instant::now();
        run_watch_loop(
            &mut source,
            roots,
            &mut debouncer,
            |root| {
                syncs.push(root.to_path_buf());
                Ok(())
            },
            || started.elapsed() >= Duration::from_millis(run_for_ms),
            Duration::from_millis(2),
        )
    }

    #[test]
    fn loop_coalesces_burst_into_one_sync() {
        let roots = vec![PathBuf::from("/r")];
        let script = (0..5)
            .map(|i| WatchEvent::Changed(PathBuf::from(format!("/r/f{i}.md"))))
            .collect();
        let mut syncs = Vec::new();
        let stats = run_fake(script, &roots, 10, 10_000, &mut syncs, 60);
        assert_eq!(stats.events, 5);
        assert_eq!(stats.sync_runs, 1);
        assert_eq!(syncs, vec![PathBuf::from("/r")]);
    }

    #[test]
    fn loop_rescan_syncs_every_root() {
        let roots = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let mut syncs = Vec::new();
        let stats = run_fake(vec![WatchEvent::Rescan], &roots, 10, 10_000, &mut syncs, 60);
        assert_eq!(stats.sync_runs, 2);
        assert_eq!(syncs, roots);
    }

    #[test]
    fn loop_routes_events_to_containing_root() {
        let roots = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let script = vec![
            WatchEvent::Changed(PathBuf::from("/b/notes.md")),
            WatchEvent::Changed(PathBuf::from("/a/code.rs")),
        ];
        let mut syncs = Vec::new();
        let stats = run_fake(script, &roots, 10, 10_000, &mut syncs, 60);
        assert_eq!(stats.sync_runs, 2);
        assert_eq!(syncs, roots); // BTreeSet order: /a before /b
    }

    #[test]
    fn loop_storm_guard_suppresses_immediate_second_sync() {
        let roots = vec![PathBuf::from("/r")];
        // Two bursts separated by more than the debounce but less than the
        // storm guard: the loop ends (stop) before the second flush is due.
        let mut script: Vec<WatchEvent> = (0..3)
            .map(|i| WatchEvent::Changed(PathBuf::from(format!("/r/f{i}.md"))))
            .collect();
        script.extend((0..3).map(|i| WatchEvent::Changed(PathBuf::from(format!("/r/g{i}.md")))));
        let mut syncs = Vec::new();
        // debounce 5 ms, storm guard 1 hour: second flush never becomes due.
        let stats = run_fake(script, &roots, 5, 3_600_000, &mut syncs, 40);
        assert_eq!(stats.sync_runs, 1);
        assert_eq!(stats.sync_failures, 0);
    }

    #[test]
    fn loop_counts_sync_failures() {
        let roots = vec![PathBuf::from("/r")];
        let mut source = FakeSource {
            script: vec![WatchEvent::Changed(PathBuf::from("/r/x.md"))].into(),
        };
        let mut debouncer =
            SyncDebouncer::new(Duration::from_millis(5), Duration::from_millis(5)).unwrap();
        let started = Instant::now();
        let stats = run_watch_loop(
            &mut source,
            &roots,
            &mut debouncer,
            |_| Err("boom".to_string()),
            || started.elapsed() >= Duration::from_millis(40),
            Duration::from_millis(2),
        );
        assert_eq!(stats.sync_runs, 1);
        assert_eq!(stats.sync_failures, 1);
    }
}
