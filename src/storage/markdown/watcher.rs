use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use super::{MarkdownVaultStorage, VaultIndexEntry, SIDECAR_SCHEMA_VERSION};
use crate::AppError;

/// Timing and batching policy for the opt-in vault watcher.
#[derive(Debug, Clone, Copy)]
pub struct VaultWatchConfig {
    pub debounce: Duration,
    pub stop_poll_interval: Duration,
}

impl Default for VaultWatchConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(250),
            stop_poll_interval: Duration::from_millis(100),
        }
    }
}

/// Work completed during one caller-owned watcher session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VaultWatchReport {
    pub batches: usize,
    pub entries_updated: usize,
    pub entries_removed: usize,
    pub full_reindexes: usize,
}

pub(super) fn watch(
    storage: &MarkdownVaultStorage,
    config: VaultWatchConfig,
    stop: impl Fn() -> bool,
) -> Result<VaultWatchReport, AppError> {
    if config.debounce.is_zero() || config.stop_poll_interval.is_zero() {
        return Err(AppError::config("markdown watcher intervals must be non-zero"));
    }

    let (send, receive) = mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        let _ = send.send(event);
    })
    .map_err(watch_error)?;
    watcher
        .watch(storage.root(), RecursiveMode::Recursive)
        .map_err(watch_error)?;

    let mut report = VaultWatchReport::default();
    while !stop() {
        let first = match receive.recv_timeout(config.stop_poll_interval) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(AppError::db("markdown watcher event channel disconnected"));
            }
        };
        let mut events = vec![first];
        loop {
            match receive.recv_timeout(config.debounce) {
                Ok(event) => events.push(event),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AppError::db("markdown watcher event channel disconnected"));
                }
            }
        }
        report.batches += 1;
        refresh_batch(storage, events, &mut report)?;
    }
    Ok(report)
}

fn refresh_batch(
    storage: &MarkdownVaultStorage,
    events: Vec<Result<Event, notify::Error>>,
    report: &mut VaultWatchReport,
) -> Result<(), AppError> {
    let mut paths = BTreeSet::new();
    let mut ambiguous = false;
    for event in events {
        match event {
            Ok(event) if matches!(event.kind, EventKind::Access(_)) => {}
            Ok(event) if event.paths.is_empty() || matches!(event.kind, EventKind::Any | EventKind::Other) => {
                ambiguous = true;
            }
            Ok(event) => {
                for path in event.paths {
                    if safe_relative_markdown_path(storage.root(), &path).is_some() {
                        paths.insert(path);
                    } else if is_ambiguous_path(storage.root(), &path) {
                        ambiguous = true;
                    }
                }
            }
            Err(_) => ambiguous = true,
        }
    }
    if ambiguous {
        storage.reindex()?;
        report.full_reindexes += 1;
        return Ok(());
    }
    if paths.is_empty() {
        return Ok(());
    }

    let mut entries = match read_sidecar(storage) {
        Ok(entries) => entries,
        Err(_) => {
            storage.reindex()?;
            report.full_reindexes += 1;
            return Ok(());
        }
    };
    for path in paths {
        let Some(relative) = safe_relative_markdown_path(storage.root(), &path) else {
            continue;
        };
        let key = slash_path(&relative)?;
        if !path.exists() {
            if entries.remove(&key).is_some() {
                report.entries_removed += 1;
            }
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            if entries.remove(&key).is_some() {
                report.entries_removed += 1;
            }
            continue;
        }
        let canonical = fs::canonicalize(&path)?;
        if !canonical.starts_with(storage.root()) {
            continue;
        }
        let document = MarkdownVaultStorage::read_document(&canonical)?;
        entries.insert(key.clone(), VaultIndexEntry::from_document(&key, &document));
        report.entries_updated += 1;
    }

    let mut ids = BTreeSet::new();
    if entries.values().any(|entry| !ids.insert(&entry.document_id)) {
        storage.reindex()?;
        report.full_reindexes += 1;
        return Ok(());
    }
    storage.write_sidecar(&entries.into_values().collect::<Vec<_>>())?;
    Ok(())
}

fn is_ambiguous_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
        })
    {
        return true;
    }
    if relative.components().any(|component| component.as_os_str() == OsStr::new(".rag"))
        || relative.file_name().is_some_and(is_temporary)
    {
        return false;
    }
    relative.extension() != Some(OsStr::new("md"))
}

fn read_sidecar(
    storage: &MarkdownVaultStorage,
) -> Result<BTreeMap<String, VaultIndexEntry>, AppError> {
    let file = fs::File::open(storage.sidecar_path())?;
    let mut entries = BTreeMap::new();
    for line in BufReader::new(file).lines() {
        let entry: VaultIndexEntry = serde_json::from_str(&line?)?;
        if entry.schema_version != SIDECAR_SCHEMA_VERSION
            || entries.insert(entry.relative_path.clone(), entry).is_some()
        {
            return Err(AppError::db("markdown sidecar is ambiguous or has the wrong version"));
        }
    }
    Ok(entries)
}

fn safe_relative_markdown_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(root).ok()?;
    if relative.components().any(|component| {
        matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
    }) || relative.components().any(|component| component.as_os_str() == OsStr::new(".rag"))
        || relative.extension() != Some(OsStr::new("md"))
        || is_temporary(relative.file_name()?)
    {
        return None;
    }
    Some(relative.to_path_buf())
}

fn is_temporary(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.')
        || name.starts_with("#")
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name.ends_with(".tmp")
}

fn slash_path(path: &Path) -> Result<String, AppError> {
    path.to_str()
        .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| AppError::config("markdown watcher path is not valid UTF-8"))
}

fn watch_error(error: notify::Error) -> AppError {
    AppError::db(format!("markdown watcher error: {error}"))
}
