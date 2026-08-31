//! Operational safety helpers: scheduled checkpointed backups and retention.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::Utc;

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

/// Spawn the optional automatic backup loop. Disabled unless
/// `RAG_AUTO_BACKUP_DIR` is configured.
pub fn spawn_auto_backup(store: Store) {
    let Some(dir) = std::env::var("RAG_AUTO_BACKUP_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return;
    };
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
    let interval = Duration::from_secs(interval_secs);
    tokio::spawn(async move {
        loop {
            let store = store.clone();
            let dir = dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                run_auto_backup_if_due(&store, &dir, interval, keep)
            })
            .await;
            match result {
                Ok(Ok(Some(path))) => {
                    tracing::info!(path = %path.display(), "automatic RAG backup created")
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => tracing::error!(error = %error, "automatic RAG backup failed"),
                Err(error) => tracing::error!(error = %error, "automatic RAG backup task panicked"),
            }
            tokio::time::sleep(interval.min(Duration::from_secs(3600))).await;
        }
    });
}

/// Create a backup only when the newest retained snapshot is older than
/// `interval`. Returns the newly created path, or `None` when not due.
pub fn run_auto_backup_if_due(
    store: &Store,
    dir: &Path,
    interval: Duration,
    keep: usize,
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

    let name = format!(
        "{BACKUP_PREFIX}{}{BACKUP_SUFFIX}",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    );
    let destination = dir.join(name);
    store.backup_database(&destination, false, false)?;

    existing = list_backups(dir)?;
    for (path, _) in existing.into_iter().skip(keep.max(1)) {
        fs::remove_file(&path).map_err(|error| {
            AppError::db(format!(
                "failed to prune old automatic backup '{}': {error}",
                path.display()
            ))
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_backup_is_due_then_skips_and_retains() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("live.duckdb");
        let backups = root.path().join("backups");
        let store = Store::open(&db).unwrap();

        let first = run_auto_backup_if_due(&store, &backups, Duration::from_secs(60), 2)
            .unwrap()
            .expect("first backup");
        assert!(first.is_file());
        assert!(
            run_auto_backup_if_due(&store, &backups, Duration::from_secs(60), 2)
                .unwrap()
                .is_none()
        );
    }
}
