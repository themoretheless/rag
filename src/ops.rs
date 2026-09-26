//! Operational safety helpers: scheduled checkpointed backups and retention.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::Instant;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Store;

pub fn wal_warn_bytes() -> u64 {
    std::env::var("RAG_WAL_WARN_BYTES").ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(256 * 1024 * 1024)
}
use crate::error::{AppError, Result};

const BACKUP_PREFIX: &str = "rag-auto-";
const BACKUP_SUFFIX: &str = ".duckdb";

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub startup_phase: String,
    pub ready: bool,
    pub pid: u32,
    pub stale_pid_metadata_detected: bool,
    pub uptime_seconds: u64,
    pub version: String,
    pub build_commit: String,
    pub tool_surface: String,
    pub http_bind: Option<String>,
    pub http_loopback_only: bool,
    pub checkpoint_ms: Option<u64>,
    pub fts_init_ms: Option<u64>,
    pub manifest_validation_ms: Option<u64>,
    pub startup_total_ms: Option<u64>,
    pub autosync_enabled: bool,
    pub autosync_running: bool,
    pub autosync_last_completed_at: Option<DateTime<Utc>>,
    pub autosync_last_error: Option<String>,
    pub autosync_next_run_at: Option<DateTime<Utc>>,
    pub auto_backup_enabled: bool,
    pub auto_backup_running: bool,
    pub auto_backup_last_completed_at: Option<DateTime<Utc>>,
    pub auto_backup_last_error: Option<String>,
    pub auto_backup_next_run_at: Option<DateTime<Utc>>,
    pub shutdown_checkpointed: bool,
}

#[derive(Debug, Clone)]
struct RuntimeState {
    phase: String,
    ready: bool,
    tool_surface: String,
    http_bind: Option<String>,
    checkpoint_ms: Option<u64>,
    fts_init_ms: Option<u64>,
    manifest_validation_ms: Option<u64>,
    startup_total_ms: Option<u64>,
    autosync_enabled: bool,
    autosync_running: bool,
    autosync_last_completed_at: Option<DateTime<Utc>>,
    autosync_last_error: Option<String>,
    autosync_next_run_at: Option<DateTime<Utc>>,
    auto_backup_enabled: bool,
    auto_backup_running: bool,
    auto_backup_last_completed_at: Option<DateTime<Utc>>,
    auto_backup_last_error: Option<String>,
    auto_backup_next_run_at: Option<DateTime<Utc>>,
    shutdown_checkpointed: bool,
    stale_pid_metadata_detected: bool,
}

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
static RUNTIME: LazyLock<RwLock<RuntimeState>> = LazyLock::new(|| RwLock::new(RuntimeState {
    phase: "process_start".into(), ready: false, tool_surface: "unknown".into(), http_bind: None,
    checkpoint_ms: None, fts_init_ms: None, manifest_validation_ms: None, startup_total_ms: None,
    autosync_enabled: false, autosync_running: false, autosync_last_completed_at: None,
    autosync_last_error: None, autosync_next_run_at: None, auto_backup_enabled: false,
    auto_backup_running: false, auto_backup_last_completed_at: None, auto_backup_last_error: None,
    auto_backup_next_run_at: None, shutdown_checkpointed: false,
    stale_pid_metadata_detected: false,
}));
static PID_PATH: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

pub fn runtime_snapshot() -> RuntimeSnapshot {
    let state = RUNTIME.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    RuntimeSnapshot {
        startup_phase: state.phase, ready: state.ready, pid: std::process::id(),
        stale_pid_metadata_detected: state.stale_pid_metadata_detected,
        uptime_seconds: STARTED.elapsed().as_secs(), version: env!("CARGO_PKG_VERSION").into(),
        build_commit: option_env!("RAG_BUILD_COMMIT").unwrap_or("unknown").into(),
        tool_surface: state.tool_surface, http_loopback_only: state.http_bind.as_deref()
            .is_none_or(|bind| bind.starts_with("127.") || bind.starts_with("[::1]") || bind.starts_with("localhost")),
        http_bind: state.http_bind, checkpoint_ms: state.checkpoint_ms, fts_init_ms: state.fts_init_ms,
        manifest_validation_ms: state.manifest_validation_ms, startup_total_ms: state.startup_total_ms,
        autosync_enabled: state.autosync_enabled, autosync_running: state.autosync_running,
        autosync_last_completed_at: state.autosync_last_completed_at,
        autosync_last_error: state.autosync_last_error, autosync_next_run_at: state.autosync_next_run_at,
        auto_backup_enabled: state.auto_backup_enabled, auto_backup_running: state.auto_backup_running,
        auto_backup_last_completed_at: state.auto_backup_last_completed_at,
        auto_backup_last_error: state.auto_backup_last_error, auto_backup_next_run_at: state.auto_backup_next_run_at,
        shutdown_checkpointed: state.shutdown_checkpointed,
    }
}

pub fn configure_runtime(tool_surface: &str, http_bind: Option<String>) {
    let mut state = RUNTIME.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.tool_surface = tool_surface.into(); state.http_bind = http_bind;
}

pub fn set_startup_phase(phase: &str) {
    RUNTIME.write().unwrap_or_else(|p| p.into_inner()).phase = phase.into();
}

pub fn set_startup_timing(name: &str, elapsed: Duration) {
    let ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
    let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner());
    match name { "checkpoint" => state.checkpoint_ms = Some(ms), "fts" => state.fts_init_ms = Some(ms),
        "manifest" => state.manifest_validation_ms = Some(ms), "total" => state.startup_total_ms = Some(ms), _ => {} }
    let warn_ms = std::env::var("RAG_STARTUP_WARN_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(30_000);
    if ms >= warn_ms { tracing::warn!(phase = name, elapsed_ms = ms, warn_ms, "slow startup phase"); }
}

pub fn mark_ready() { let mut s = RUNTIME.write().unwrap_or_else(|p| p.into_inner()); s.phase = "ready".into(); s.ready = true; }
pub fn mark_shutdown(checkpointed: bool) {
    let mut s = RUNTIME.write().unwrap_or_else(|p| p.into_inner()); s.phase = "shutdown_complete".into(); s.ready = false; s.shutdown_checkpointed = checkpointed;
    drop(s);
    if let Some(path) = PID_PATH.read().unwrap_or_else(|p| p.into_inner()).as_ref() { let _ = fs::remove_file(path); }
}

pub fn configure_autosync(interval: Duration) {
    let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner());
    state.autosync_enabled = true;
    state.autosync_next_run_at = Some(Utc::now());
    drop(state);
    set_autosync_next(interval);
}

pub fn mark_autosync_running() { RUNTIME.write().unwrap_or_else(|p| p.into_inner()).autosync_running = true; }
pub fn mark_autosync_success(interval: Duration) {
    let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner());
    state.autosync_running = false; state.autosync_last_completed_at = Some(Utc::now()); state.autosync_last_error = None;
    state.autosync_next_run_at = Some(Utc::now() + chrono::Duration::from_std(interval).unwrap_or_default());
}
pub fn mark_autosync_error(error: &str, interval: Duration) {
    let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner());
    state.autosync_running = false; state.autosync_last_error = Some(error.into());
    state.autosync_next_run_at = Some(Utc::now() + chrono::Duration::from_std(interval).unwrap_or_default());
}
fn set_autosync_next(interval: Duration) {
    RUNTIME.write().unwrap_or_else(|p| p.into_inner()).autosync_next_run_at =
        Some(Utc::now() + chrono::Duration::from_std(interval).unwrap_or_default());
}

pub fn validate_runtime_paths(db_path: &Path, ingest_roots: &[PathBuf]) -> Result<()> {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() { fs::create_dir_all(parent)?; }
    if fs::metadata(parent)?.permissions().readonly() { return Err(AppError::config(format!("database directory is read-only: {}", parent.display()))); }
    for root in ingest_roots {
        if !root.is_dir() { return Err(AppError::config(format!("ingest root is not a readable directory: {}", root.display()))); }
        // Do not enumerate protected macOS folders during launchd startup: TCC can
        // block a background process indefinitely before the HTTP listener opens.
        // Individual sync/ingest calls still perform and report the real read.
    }
    if let Some(dir) = auto_backup_dir() {
        fs::create_dir_all(&dir)?;
        if fs::metadata(&dir)?.permissions().readonly() { return Err(AppError::config(format!("auto-backup directory is read-only: {}", dir.display()))); }
    }
    let pid_path = std::env::var("RAG_PID_FILE").ok().filter(|v| !v.trim().is_empty())
        .map(PathBuf::from).unwrap_or_else(|| db_path.with_extension("pid"));
    let stale = fs::read_to_string(&pid_path).ok().map(|v| v.trim() != std::process::id().to_string()).unwrap_or(false);
    fs::write(&pid_path, format!("{}\n", std::process::id()))?;
    *PID_PATH.write().unwrap_or_else(|p| p.into_inner()) = Some(pid_path);
    RUNTIME.write().unwrap_or_else(|p| p.into_inner()).stale_pid_metadata_detected = stale;
    Ok(())
}

fn auto_backup_dir() -> Option<PathBuf> {
    std::env::var("RAG_AUTO_BACKUP_DIR").ok().map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Snapshot-loop settings, read together so the running loop and the reported
/// inventory cannot disagree about what `keep` is.
#[derive(Debug, Clone)]
struct AutoBackupSettings {
    dir: PathBuf,
    interval: Duration,
    keep: usize,
    min_free_bytes: u64,
}

fn auto_backup_settings() -> Option<AutoBackupSettings> {
    let dir = auto_backup_dir()?;
    let interval_secs = std::env::var("RAG_AUTO_BACKUP_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(86_400)
        .max(1);
    let keep = std::env::var("RAG_AUTO_BACKUP_KEEP")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(7)
        .max(1);
    let min_free_bytes = std::env::var("RAG_AUTO_BACKUP_MIN_FREE_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(2 * 1024 * 1024 * 1024);
    Some(AutoBackupSettings {
        dir,
        interval: Duration::from_secs(interval_secs),
        keep,
        min_free_bytes,
    })
}

/// Free bytes usable by an unprivileged process on the volume holding `path`.
/// `None` means the platform cannot answer, which callers treat as unknown
/// rather than as "no room".
#[cfg(unix)]
pub fn free_space_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // `f_frsize` is the fragment size; some platforms leave it zero and only
    // populate the "optimal transfer" `f_bsize`.
    let block_size = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    } as u64;
    Some(stat.f_bavail as u64 * block_size)
}

#[cfg(not(unix))]
pub fn free_space_bytes(_path: &Path) -> Option<u64> {
    None
}

/// Automatic snapshot footprint: what retention currently holds and what it
/// costs. `status` reports it so the disk budget is readable without a shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBackupInventory {
    pub dir: String,
    pub interval_secs: u64,
    pub keep: usize,
    /// Headroom the guard requires on top of the snapshot itself.
    pub min_free_bytes: u64,
    pub snapshots: usize,
    pub total_bytes: u64,
    pub newest_at: Option<DateTime<Utc>>,
    pub free_bytes: Option<u64>,
}

pub fn auto_backup_inventory() -> Option<AutoBackupInventory> {
    let settings = auto_backup_settings()?;
    Some(inventory_at(&settings))
}

fn inventory_at(settings: &AutoBackupSettings) -> AutoBackupInventory {
    let snapshots = list_backups(&settings.dir).unwrap_or_default();
    // Sidecars are billed too: retention prunes them with their snapshot.
    let total_bytes = snapshots.iter().fold(0u64, |acc, (path, _)| {
        let mut group = vec![path.clone()];
        group.push(PathBuf::from(format!("{}.sha256", path.display())));
        group.push(PathBuf::from(format!("{}.metadata.json", path.display())));
        group.into_iter().fold(acc, |acc, file| {
            acc.saturating_add(fs::metadata(file).map(|meta| meta.len()).unwrap_or(0))
        })
    });
    AutoBackupInventory {
        dir: settings.dir.display().to_string(),
        interval_secs: settings.interval.as_secs(),
        keep: settings.keep,
        min_free_bytes: settings.min_free_bytes,
        snapshots: snapshots.len(),
        total_bytes,
        newest_at: snapshots.first().map(|(_, m)| DateTime::<Utc>::from(*m)),
        free_bytes: free_space_bytes(&settings.dir),
    }
}

/// Spawn the optional automatic backup loop. Disabled unless
/// `RAG_AUTO_BACKUP_DIR` is configured.
pub fn spawn_auto_backup(store: Store) {
    let Some(settings) = auto_backup_settings() else {
        return;
    };
    let dir = settings.dir.clone();
    let interval = settings.interval;
    let keep = settings.keep;
    let min_free_bytes = settings.min_free_bytes;
    {
        let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner());
        state.auto_backup_enabled = true;
        state.auto_backup_last_completed_at = latest_auto_backup_at(&dir).ok().flatten();
        state.auto_backup_next_run_at = state.auto_backup_last_completed_at
            .map(|completed| completed + chrono::Duration::from_std(interval).unwrap_or_default())
            .or_else(|| Some(Utc::now()));
    }
    tokio::spawn(async move {
        loop {
            RUNTIME.write().unwrap_or_else(|p| p.into_inner()).auto_backup_running = true;
            let store = store.clone();
            let dir = dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                run_auto_backup_if_due(&store, &dir, interval, keep, min_free_bytes)
            })
            .await;
            match result {
                Ok(Ok(Some(path))) => {
                    let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner());
                    state.auto_backup_last_completed_at = Some(Utc::now());
                    state.auto_backup_last_error = None;
                    tracing::info!(path = %path.display(), "automatic RAG backup created")
                }
                Ok(Ok(None)) => { RUNTIME.write().unwrap_or_else(|p| p.into_inner()).auto_backup_last_error = None; }
                Ok(Err(error)) => { RUNTIME.write().unwrap_or_else(|p| p.into_inner()).auto_backup_last_error = Some(error.to_string()); tracing::error!(error = %error, "automatic RAG backup failed") }
                Err(error) => { RUNTIME.write().unwrap_or_else(|p| p.into_inner()).auto_backup_last_error = Some(error.to_string()); tracing::error!(error = %error, "automatic RAG backup task panicked") }
            }
            { let mut state = RUNTIME.write().unwrap_or_else(|p| p.into_inner()); state.auto_backup_running = false;
              state.auto_backup_next_run_at = Some(Utc::now() + chrono::Duration::from_std(interval.min(Duration::from_secs(3600))).unwrap_or_default()); }
            tokio::time::sleep(interval.min(Duration::from_secs(3600))).await;
        }
    });
}

/// Create a backup only when the newest retained snapshot is older than
/// `interval`. Returns the newly created path, or `None` when not due.
///
/// Refuses to write when the volume would not hold the snapshot and keep
/// `min_free_bytes` afterwards: an unattended loop that fills the disk takes
/// the store down with it.
pub fn run_auto_backup_if_due(
    store: &Store,
    dir: &Path,
    interval: Duration,
    keep: usize,
    min_free_bytes: u64,
) -> Result<Option<PathBuf>> {
    fs::create_dir_all(dir)?;
    let mut existing = list_backups(dir)?;
    if let Some((_, modified)) = existing.first() {
        if SystemTime::now()
            .duration_since(*modified)
            .unwrap_or_default()
            < interval
        {
            return Ok(None);
        }
    }

    // A CHECKPOINT snapshot is about the live file, which still carries its free
    // pages, so this estimate is deliberately larger than the copy it produces.
    let needed_bytes = store
        .db_file_size_bytes()
        .unwrap_or_default()
        .saturating_add(min_free_bytes);
    if let Some(free) = free_space_bytes(dir) {
        if free < needed_bytes {
            return Err(AppError::config(format!(
                "refusing automatic backup to '{}': volume has {free} free bytes, needs \
                 {needed_bytes} for a snapshot plus {min_free_bytes} headroom; raise \
                 RAG_AUTO_BACKUP_MIN_FREE_BYTES or lower RAG_AUTO_BACKUP_KEEP (now {keep})",
                dir.display()
            )));
        }
    }

    let name = format!(
        "{BACKUP_PREFIX}{}{BACKUP_SUFFIX}",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    );
    let mut destination = dir.join(&name);
    let mut suffix = 1u32;
    while destination.exists() {
        destination = dir.join(format!(
            "{}-{suffix}{BACKUP_SUFFIX}",
            name.trim_end_matches(BACKUP_SUFFIX)
        ));
        suffix += 1;
    }
    store.backup_database(&destination, false, false)?;

    existing = list_backups(dir)?;
    for (path, _) in existing.into_iter().skip(keep.max(1)) {
        fs::remove_file(&path).map_err(|error| {
            AppError::db(format!(
                "failed to prune old automatic backup '{}': {error}",
                path.display()
            ))
        })?;
        for sidecar in [
            PathBuf::from(format!("{}.sha256", path.display())),
            PathBuf::from(format!("{}.metadata.json", path.display())),
        ] {
            if sidecar.exists() {
                fs::remove_file(&sidecar).map_err(|error| {
                    AppError::db(format!(
                        "failed to prune automatic backup sidecar '{}': {error}",
                        sidecar.display()
                    ))
                })?;
            }
        }
    }
    Ok(Some(destination))
}

fn list_backups(dir: &Path) -> Result<Vec<(PathBuf, SystemTime)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(BACKUP_PREFIX) || !name.ends_with(BACKUP_SUFFIX) {
            continue;
        }
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        out.push((path, modified));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    Ok(out)
}

fn latest_auto_backup_at(dir: &Path) -> Result<Option<DateTime<Utc>>> {
    Ok(list_backups(dir)?.first().map(|(_, modified)| DateTime::<Utc>::from(*modified)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_backup_is_due_then_skips_and_retains() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("live.duckdb");
        let backups = root.path().join("backups");
        let store = Store::open(&db).unwrap();

        let first = run_auto_backup_if_due(&store, &backups, Duration::from_secs(60), 2, 0)
            .unwrap()
            .expect("first backup");
        assert!(first.is_file());
        assert!(
            run_auto_backup_if_due(&store, &backups, Duration::from_secs(60), 2, 0)
                .unwrap()
                .is_none()
        );
        let completed = latest_auto_backup_at(&backups).unwrap().expect("latest backup time");
        assert!(completed <= Utc::now());
    }

    #[test]
    #[cfg(unix)]
    fn auto_backup_refuses_when_the_volume_cannot_hold_it() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("live.duckdb");
        let backups = root.path().join("backups");
        let store = Store::open(&db).unwrap();

        // Larger than any real volume: the simulated almost-full-disk case.
        let error =
            run_auto_backup_if_due(&store, &backups, Duration::from_secs(60), 7, u64::MAX / 2)
                .unwrap_err();
        assert!(
            error.to_string().contains("refusing automatic backup"),
            "{error}"
        );
        assert!(
            list_backups(&backups).unwrap().is_empty(),
            "a refused snapshot must not leave a file behind"
        );
    }

    #[test]
    fn auto_backup_inventory_counts_snapshots_that_retention_would_keep() {
        let root = tempfile::tempdir().unwrap();
        let backups = root.path().join("backups");
        fs::create_dir_all(&backups).unwrap();
        fs::write(
            backups.join(format!("{BACKUP_PREFIX}old{BACKUP_SUFFIX}")),
            b"12345",
        )
        .unwrap();
        fs::write(
            backups.join(format!("{BACKUP_PREFIX}new{BACKUP_SUFFIX}")),
            b"1234567890",
        )
        .unwrap();
        fs::write(
            backups.join(format!("{BACKUP_PREFIX}new{BACKUP_SUFFIX}.sha256")),
            b"abc",
        )
        .unwrap();
        // Retention-unrelated noise must not be billed to the snapshot budget.
        fs::write(
            backups.join("handmade-copy.duckdb"),
            b"00000000000000000000",
        )
        .unwrap();

        let settings = AutoBackupSettings {
            dir: backups.clone(),
            interval: Duration::from_secs(86_400),
            keep: 7,
            min_free_bytes: 2 * 1024 * 1024 * 1024,
        };
        let inventory = inventory_at(&settings);
        assert_eq!(inventory.snapshots, 2);
        assert_eq!(inventory.total_bytes, 18);
        assert_eq!(inventory.keep, 7);
        assert_eq!(inventory.interval_secs, 86_400);
        assert!(inventory.newest_at.is_some(), "newest snapshot timestamp");
        assert!(inventory.free_bytes.is_some(), "volume headroom");
        assert_eq!(inventory.dir, backups.display().to_string());

        // An empty configured directory still reports its settings.
        let empty = root.path().join("nothing-yet");
        let blank = inventory_at(&AutoBackupSettings {
            dir: empty,
            ..settings
        });
        assert_eq!(blank.snapshots, 0);
        assert_eq!(blank.total_bytes, 0);
        assert!(blank.newest_at.is_none());
    }

    #[test]
    fn runtime_snapshot_exposes_build_and_operational_shape() {
        configure_runtime("spine", Some("127.0.0.1:7432".into()));
        let snapshot = runtime_snapshot();
        assert_eq!(snapshot.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(snapshot.tool_surface, "spine");
        assert!(snapshot.http_loopback_only);
        assert!(snapshot.pid > 0);
    }

    #[test]
    fn runtime_path_validation_writes_and_detects_stale_pid_metadata() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("rag.duckdb");
        let pid = db.with_extension("pid");
        fs::write(&pid, "999999\n").unwrap();
        validate_runtime_paths(&db, &[root.path().to_path_buf()]).unwrap();
        assert!(runtime_snapshot().stale_pid_metadata_detected);
        assert_eq!(fs::read_to_string(pid).unwrap().trim(), std::process::id().to_string());
    }
}
