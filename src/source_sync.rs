//! Application service for synchronizing a filesystem tree into the corpus.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::db::{SourceManifestEntry, SourceManifestWrite, Store};
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::file_ingest::{extract_file, is_supported_source, merge_metadata};
use crate::ingest::{IngestCommand, IngestService, PreparedIngest, SourceManifestStamp};
use crate::source_scan::{collect_source_files_while, SourceScanPolicy};
use crate::util::{check_path_allowlist, content_hash};

/// Default safety cap for one file in recursive source synchronization.
pub const DEFAULT_SOURCE_SYNC_MAX_FILE_BYTES: u64 = 512 * 1024;

const SOURCE_SYNC_BATCH_MAX_DOCUMENTS: usize = 64;
const SOURCE_SYNC_BATCH_MAX_CHUNKS: usize = 64;

#[derive(Debug, Clone)]
pub struct SourceSyncCommand {
    pub path: PathBuf,
    pub remove_deleted: bool,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub max_file_bytes: Option<u64>,
}

impl SourceSyncCommand {
    /// Stable minimal constructor; optional scope, deletion, and size controls
    /// can be set explicitly by callers that need them.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            remove_deleted: false,
            wing: None,
            room: None,
            max_file_bytes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceSyncError {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceSyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
    pub deleted: Vec<String>,
    pub errors: Vec<SourceSyncError>,
    pub counters: SourceSyncCounters,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceSyncCounters {
    pub preflight: u64,
    pub extracted: u64,
    pub embedded: u64,
}

impl SourceSyncReport {
    fn sort_paths(&mut self) {
        self.added.sort();
        self.updated.sort();
        self.skipped.sort();
        self.deleted.sort();
        self.errors
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
}

/// Observable phases for a cooperative source synchronization run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSyncPhase {
    Scanning,
    Syncing,
    RemovingDeleted,
    Completed,
    Cancelled,
}

/// Progress snapshot emitted while a source tree is synchronized.
#[derive(Debug, Clone, Serialize)]
pub struct SourceSyncProgress {
    pub phase: SourceSyncPhase,
    pub total_files: usize,
    pub processed_files: usize,
    pub current_path: Option<String>,
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub errors: usize,
    pub counters: SourceSyncCounters,
}

impl SourceSyncProgress {
    fn from_report(
        phase: SourceSyncPhase,
        total_files: usize,
        processed_files: usize,
        current_path: Option<String>,
        report: &SourceSyncReport,
    ) -> Self {
        Self {
            phase,
            total_files,
            processed_files,
            current_path,
            added: report.added.len(),
            updated: report.updated.len(),
            skipped: report.skipped.len(),
            deleted: report.deleted.len(),
            errors: report.errors.len(),
            counters: report.counters.clone(),
        }
    }
}

/// Result of a controlled run. Cancellation preserves the partial report.
#[derive(Debug, Clone)]
pub enum SourceSyncOutcome {
    Completed(SourceSyncReport),
    Cancelled(SourceSyncReport),
}

/// Cancellation and progress hooks used by background jobs.
#[derive(Clone)]
pub struct SourceSyncControl {
    cancellation: CancellationToken,
    on_progress: Arc<dyn Fn(SourceSyncProgress) + Send + Sync>,
}

impl Default for SourceSyncControl {
    fn default() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            on_progress: Arc::new(|_| {}),
        }
    }
}

impl SourceSyncControl {
    pub fn new(
        cancellation: CancellationToken,
        on_progress: impl Fn(SourceSyncProgress) + Send + Sync + 'static,
    ) -> Self {
        Self {
            cancellation,
            on_progress: Arc::new(on_progress),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    fn publish(&self, progress: SourceSyncProgress) {
        (self.on_progress)(progress);
    }

    async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
}

pub struct SourceSyncService<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
}

struct SourceSyncRunContext<'a> {
    root: &'a Path,
    command: &'a SourceSyncCommand,
    scan_policy: &'a SourceScanPolicy,
    preloaded: &'a HashMap<String, SourceManifestEntry>,
}

struct PendingSourceIngest {
    file_index: usize,
    source_file: String,
    prepared: PreparedIngest,
}

enum PreparedSourceFile {
    Skipped {
        source_file: String,
        manifest_refresh: Option<SourceManifestRefresh>,
    },
    Ingest {
        source_file: String,
        prepared: PreparedIngest,
    },
}

struct SourceManifestRefresh {
    stamp: SourceManifestStamp,
    content_hash: String,
    document_id: String,
}

impl SourceManifestRefresh {
    fn commit(&self, store: &Store) -> Result<(), AppError> {
        store.upsert_source_manifest(SourceManifestWrite {
            canonical_path: &self.stamp.canonical_path,
            canonical_root: &self.stamp.canonical_root,
            size_bytes: self.stamp.size_bytes,
            mtime_ns: self.stamp.mtime_ns,
            content_hash: &self.content_hash,
            document_id: &self.document_id,
        })
    }
}

enum BatchFlushOutcome {
    Completed(usize),
    Cancelled(usize),
}

impl<'a> SourceSyncService<'a> {
    pub fn new(
        store: &'a Store,
        embedder: &'a Arc<dyn EmbeddingProvider>,
        config: &'a Config,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    pub async fn sync(&self, command: SourceSyncCommand) -> Result<SourceSyncReport, AppError> {
        match self
            .sync_with_control(command, SourceSyncControl::default())
            .await?
        {
            SourceSyncOutcome::Completed(report) => Ok(report),
            SourceSyncOutcome::Cancelled(report) => Ok(report),
        }
    }

    /// Synchronize with cooperative cancellation and progress reporting.
    pub async fn sync_with_control(
        &self,
        command: SourceSyncCommand,
        control: SourceSyncControl,
    ) -> Result<SourceSyncOutcome, AppError> {
        let mut report = SourceSyncReport::default();
        let sync_lane = self.store.source_sync_lane();
        let _sync_guard = tokio::select! {
            guard = sync_lane.lock_owned() => guard,
            () = control.cancelled() => {
                return Ok(cancelled_outcome(&control, report, 0, 0));
            }
        };
        control.publish(SourceSyncProgress::from_report(
            SourceSyncPhase::Scanning,
            0,
            0,
            None,
            &report,
        ));
        if control.is_cancelled() {
            return Ok(cancelled_outcome(&control, report, 0, 0));
        }
        check_path_allowlist(&command.path, &self.config.ingest_roots)?;
        let root = command.path.canonicalize().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found(format!("directory not found: {}", command.path.display()))
            } else {
                AppError::from(error)
            }
        })?;
        if !root.is_dir() {
            return Err(AppError::config(format!(
                "sync_sources path is not a directory: {}",
                root.display()
            )));
        }

        let scan_policy = SourceScanPolicy::default().with_max_bytes(
            command
                .max_file_bytes
                .unwrap_or(DEFAULT_SOURCE_SYNC_MAX_FILE_BYTES)
                .max(1),
        );
        // Discover every supported source first. The size guard is enforced in
        // `source_stamp` so oversized files become visible report errors rather
        // than silently retaining stale indexed content.
        let discovery_policy = SourceScanPolicy::default();
        let files =
            collect_source_files_while(&root, &discovery_policy, || !control.is_cancelled())?;
        let total_files = files.len();
        let preloaded = self.store.load_source_manifest_root(&root)?;
        let run = SourceSyncRunContext {
            root: &root,
            command: &command,
            scan_policy: &scan_policy,
            preloaded: &preloaded,
        };
        let mut seen = BTreeSet::new();
        if control.is_cancelled() {
            return Ok(cancelled_outcome(&control, report, total_files, 0));
        }
        let mut pending = Vec::new();
        let mut pending_chunks = 0usize;
        let mut processed_files = 0usize;
        for (index, path) in files.into_iter().enumerate() {
            control.publish(SourceSyncProgress::from_report(
                SourceSyncPhase::Syncing,
                total_files,
                processed_files,
                Some(path.display().to_string()),
                &report,
            ));
            if control.is_cancelled() {
                return Ok(cancelled_outcome(
                    &control,
                    report,
                    total_files,
                    processed_files,
                ));
            }
            report.counters.preflight = report.counters.preflight.saturating_add(1);

            let preparation = self.prepare_source_file(&run, &path, &mut seen, &mut report);
            if control.is_cancelled() {
                return Ok(cancelled_outcome(
                    &control,
                    report,
                    total_files,
                    processed_files,
                ));
            }

            match preparation {
                Ok(PreparedSourceFile::Ingest {
                    source_file,
                    prepared,
                }) => {
                    let chunk_count = prepared.chunk_count();
                    let exceeds_current_batch = !pending.is_empty()
                        && (pending.len() >= SOURCE_SYNC_BATCH_MAX_DOCUMENTS
                            || chunk_count > SOURCE_SYNC_BATCH_MAX_CHUNKS
                            || pending_chunks.saturating_add(chunk_count)
                                > SOURCE_SYNC_BATCH_MAX_CHUNKS);
                    if exceeds_current_batch {
                        match self
                            .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                            .await
                        {
                            BatchFlushOutcome::Completed(processed) => {
                                processed_files = processed;
                                pending_chunks = 0;
                                if control.is_cancelled() {
                                    return Ok(cancelled_outcome(
                                        &control,
                                        report,
                                        total_files,
                                        processed,
                                    ));
                                }
                            }
                            BatchFlushOutcome::Cancelled(processed) => {
                                return Ok(cancelled_outcome(
                                    &control,
                                    report,
                                    total_files,
                                    processed,
                                ));
                            }
                        }
                    }

                    pending_chunks = pending_chunks.saturating_add(chunk_count);
                    pending.push(PendingSourceIngest {
                        file_index: index,
                        source_file,
                        prepared,
                    });
                    if pending.len() >= SOURCE_SYNC_BATCH_MAX_DOCUMENTS
                        || pending_chunks >= SOURCE_SYNC_BATCH_MAX_CHUNKS
                        || chunk_count > SOURCE_SYNC_BATCH_MAX_CHUNKS
                    {
                        match self
                            .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                            .await
                        {
                            BatchFlushOutcome::Completed(processed) => {
                                processed_files = processed;
                                pending_chunks = 0;
                            }
                            BatchFlushOutcome::Cancelled(processed) => {
                                return Ok(cancelled_outcome(
                                    &control,
                                    report,
                                    total_files,
                                    processed,
                                ));
                            }
                        }
                    }
                }
                Ok(PreparedSourceFile::Skipped {
                    source_file,
                    manifest_refresh,
                }) => {
                    if !pending.is_empty() {
                        match self
                            .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                            .await
                        {
                            BatchFlushOutcome::Completed(processed) => {
                                pending_chunks = 0;
                                if control.is_cancelled() {
                                    return Ok(cancelled_outcome(
                                        &control,
                                        report,
                                        total_files,
                                        processed,
                                    ));
                                }
                            }
                            BatchFlushOutcome::Cancelled(processed) => {
                                return Ok(cancelled_outcome(
                                    &control,
                                    report,
                                    total_files,
                                    processed,
                                ));
                            }
                        }
                    }
                    let result = manifest_refresh
                        .as_ref()
                        .map_or(Ok(()), |refresh| refresh.commit(self.store));
                    match result {
                        Ok(()) => report.skipped.push(source_file),
                        Err(error) => report.errors.push(SourceSyncError {
                            path: source_file,
                            error: error.to_string(),
                        }),
                    }
                    processed_files = index + 1;
                    publish_file_completion(&control, &report, total_files, processed_files);
                }
                Err(error) => {
                    if !pending.is_empty() {
                        match self
                            .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                            .await
                        {
                            BatchFlushOutcome::Completed(_) => {
                                pending_chunks = 0;
                            }
                            BatchFlushOutcome::Cancelled(processed) => {
                                return Ok(cancelled_outcome(
                                    &control,
                                    report,
                                    total_files,
                                    processed,
                                ));
                            }
                        }
                    }
                    report.errors.push(SourceSyncError {
                        path: path.display().to_string(),
                        error: error.to_string(),
                    });
                    processed_files = index + 1;
                    publish_file_completion(&control, &report, total_files, processed_files);
                }
            }

            if control.is_cancelled() {
                return Ok(cancelled_outcome(
                    &control,
                    report,
                    total_files,
                    processed_files,
                ));
            }
        }
        if !pending.is_empty() {
            match self
                .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                .await
            {
                BatchFlushOutcome::Completed(processed) => {
                    processed_files = processed;
                }
                BatchFlushOutcome::Cancelled(processed) => {
                    return Ok(cancelled_outcome(&control, report, total_files, processed));
                }
            }
        }
        if control.is_cancelled() {
            return Ok(cancelled_outcome(
                &control,
                report,
                total_files,
                processed_files,
            ));
        }
        if command.remove_deleted {
            control.publish(SourceSyncProgress::from_report(
                SourceSyncPhase::RemovingDeleted,
                total_files,
                total_files,
                None,
                &report,
            ));
            if control.is_cancelled() {
                return Ok(cancelled_outcome(
                    &control,
                    report,
                    total_files,
                    total_files,
                ));
            }
            if self.remove_deleted(&seen, &preloaded, &mut report, &control, total_files)? {
                return Ok(cancelled_outcome(
                    &control,
                    report,
                    total_files,
                    total_files,
                ));
            }
        }
        if control.is_cancelled() {
            return Ok(cancelled_outcome(
                &control,
                report,
                total_files,
                total_files,
            ));
        }
        let seen_paths = seen.iter().cloned().collect::<Vec<_>>();
        self.store.mark_source_manifest_seen(&root, &seen_paths)?;
        report.sort_paths();
        control.publish(SourceSyncProgress::from_report(
            SourceSyncPhase::Completed,
            total_files,
            total_files,
            None,
            &report,
        ));
        Ok(SourceSyncOutcome::Completed(report))
    }

    fn prepare_source_file(
        &self,
        run: &SourceSyncRunContext<'_>,
        path: &Path,
        seen: &mut BTreeSet<String>,
        report: &mut SourceSyncReport,
    ) -> Result<PreparedSourceFile, AppError> {
        let canonical = path.canonicalize()?;
        let source_file = canonical.display().to_string();
        let canonical_root = run.root.display().to_string();
        seen.insert(source_file.clone());
        let stamp = source_stamp(&canonical, run.scan_policy)?;
        let existing = run.preloaded.get(&source_file);
        if existing.is_some_and(|entry| {
            manifest_preflight_matches(entry, stamp, &canonical_root)
                && requested_scope_matches(entry, run.command)
        }) {
            return Ok(PreparedSourceFile::Skipped {
                source_file,
                manifest_refresh: None,
            });
        }

        let uri = format!("file://{}", canonical.display());
        let extracted =
            extract_file(&canonical).map_err(|error| AppError::config(error.to_string()))?;
        report.counters.extracted = report.counters.extracted.saturating_add(1);
        let hash = content_hash(&extracted.text);
        if let Some(entry) = existing.filter(|entry| {
            manifest_content_matches(entry, &hash) && requested_scope_matches(entry, run.command)
        }) {
            return Ok(PreparedSourceFile::Skipped {
                source_file: source_file.clone(),
                manifest_refresh: Some(SourceManifestRefresh {
                    stamp: SourceManifestStamp {
                        canonical_path: source_file,
                        canonical_root,
                        size_bytes: stamp.size_bytes,
                        mtime_ns: stamp.mtime_ns,
                    },
                    content_hash: hash,
                    document_id: entry.document_id.clone(),
                }),
            });
        }

        let metadata_json = merge_metadata(
            existing.and_then(|entry| entry.document_metadata_json.clone()),
            extracted.metadata,
        )
        .map_err(|error| AppError::config(error.to_string()))?;
        let (inferred_wing, inferred_room) = inferred_scope(run.root, &canonical);
        let prepared = IngestService::new(self.store, self.embedder, self.config).prepare_source(
            IngestCommand {
                text: extracted.text,
                title: canonical
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string),
                uri: Some(uri),
                metadata_json: Some(metadata_json),
                wing: requested_scope(run.command.wing.as_deref())
                    .map(str::to_string)
                    .or_else(|| existing.and_then(|entry| entry.document_wing.clone()))
                    .or(Some(inferred_wing)),
                room: requested_scope(run.command.room.as_deref())
                    .map(str::to_string)
                    .or_else(|| existing.and_then(|entry| entry.document_room.clone()))
                    .or(Some(inferred_room)),
                source_file: Some(source_file.clone()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            },
            SourceManifestStamp {
                canonical_path: source_file.clone(),
                canonical_root,
                size_bytes: stamp.size_bytes,
                mtime_ns: stamp.mtime_ns,
            },
        )?;
        Ok(PreparedSourceFile::Ingest {
            source_file,
            prepared,
        })
    }

    async fn flush_pending_batch(
        &self,
        pending: &mut Vec<PendingSourceIngest>,
        control: &SourceSyncControl,
        report: &mut SourceSyncReport,
        total_files: usize,
    ) -> BatchFlushOutcome {
        debug_assert!(!pending.is_empty());
        let batch = std::mem::take(pending);
        let first_uncommitted = batch[0].file_index;
        let positions = batch
            .iter()
            .map(|item| (item.file_index, item.source_file.clone()))
            .collect::<Vec<_>>();
        let prepared = batch
            .into_iter()
            .map(|item| item.prepared)
            .collect::<Vec<_>>();

        if control.is_cancelled() {
            return BatchFlushOutcome::Cancelled(first_uncommitted);
        }
        let ingest = IngestService::new(self.store, self.embedder, self.config);
        let embedding = ingest.embed_prepared_batch(prepared);
        let embedded = tokio::select! {
            biased;
            () = control.cancelled() => {
                return BatchFlushOutcome::Cancelled(first_uncommitted);
            }
            result = embedding => result,
        };

        let embedded = match embedded {
            Ok(embedded) => embedded,
            Err(error) => {
                let error = error.to_string();
                let mut processed_files = first_uncommitted;
                for (file_index, source_file) in positions {
                    report.errors.push(SourceSyncError {
                        path: source_file,
                        error: error.clone(),
                    });
                    processed_files = file_index + 1;
                    publish_file_completion(control, report, total_files, processed_files);
                    if control.is_cancelled() {
                        return BatchFlushOutcome::Cancelled(processed_files);
                    }
                }
                return BatchFlushOutcome::Completed(processed_files);
            }
        };

        if control.is_cancelled() {
            return BatchFlushOutcome::Cancelled(first_uncommitted);
        }
        debug_assert_eq!(positions.len(), embedded.len());
        let mut processed_files = first_uncommitted;
        for ((file_index, source_file), embedded) in positions.into_iter().zip(embedded) {
            if control.is_cancelled() {
                return BatchFlushOutcome::Cancelled(processed_files);
            }
            match ingest.commit_embedded(embedded) {
                Ok(result) => {
                    if result.chunk_count > 0 {
                        report.counters.embedded = report.counters.embedded.saturating_add(1);
                    }
                    if result.op == "inserted" {
                        report.added.push(source_file);
                    } else {
                        report.updated.push(source_file);
                    }
                }
                Err(error) => report.errors.push(SourceSyncError {
                    path: source_file,
                    error: error.to_string(),
                }),
            }
            processed_files = file_index + 1;
            publish_file_completion(control, report, total_files, processed_files);
            if control.is_cancelled() {
                return BatchFlushOutcome::Cancelled(processed_files);
            }
        }
        BatchFlushOutcome::Completed(processed_files)
    }

    fn remove_deleted(
        &self,
        seen: &BTreeSet<String>,
        preloaded: &HashMap<String, SourceManifestEntry>,
        report: &mut SourceSyncReport,
        control: &SourceSyncControl,
        total_files: usize,
    ) -> Result<bool, AppError> {
        self.remove_deleted_with_metadata(seen, preloaded, report, control, total_files, |path| {
            std::fs::metadata(path)
        })
    }

    fn remove_deleted_with_metadata(
        &self,
        seen: &BTreeSet<String>,
        preloaded: &HashMap<String, SourceManifestEntry>,
        report: &mut SourceSyncReport,
        control: &SourceSyncControl,
        total_files: usize,
        metadata: impl Fn(&Path) -> std::io::Result<std::fs::Metadata>,
    ) -> Result<bool, AppError> {
        let mut sources = preloaded.keys().collect::<Vec<_>>();
        sources.sort();
        for source in sources {
            control.publish(SourceSyncProgress::from_report(
                SourceSyncPhase::RemovingDeleted,
                total_files,
                total_files,
                Some(source.clone()),
                report,
            ));
            if control.is_cancelled() {
                return Ok(true);
            }
            let path = Path::new(&source);
            if !is_supported_source(path) || seen.contains(source) {
                continue;
            }
            let missing = match metadata_means_missing(metadata(path)) {
                Ok(missing) => missing,
                Err(error) => {
                    report.errors.push(SourceSyncError {
                        path: source.clone(),
                        error: format!(
                            "cannot determine whether source was deleted; preserving indexed state: {error}"
                        ),
                    });
                    continue;
                }
            };
            if missing {
                let (deleted_documents, deleted_manifest) =
                    match self.store.delete_source_state(source) {
                        Ok(deleted) => deleted,
                        Err(error) => {
                            report.errors.push(SourceSyncError {
                                path: source.clone(),
                                error: error.to_string(),
                            });
                            continue;
                        }
                    };
                if deleted_documents > 0 || deleted_manifest {
                    report.deleted.push(source.clone());
                }
            }
        }
        Ok(false)
    }
}

fn metadata_means_missing(metadata: std::io::Result<std::fs::Metadata>) -> std::io::Result<bool> {
    match metadata {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

fn cancelled_outcome(
    control: &SourceSyncControl,
    mut report: SourceSyncReport,
    total_files: usize,
    processed_files: usize,
) -> SourceSyncOutcome {
    report.sort_paths();
    control.publish(SourceSyncProgress::from_report(
        SourceSyncPhase::Cancelled,
        total_files,
        processed_files,
        None,
        &report,
    ));
    SourceSyncOutcome::Cancelled(report)
}

fn publish_file_completion(
    control: &SourceSyncControl,
    report: &SourceSyncReport,
    total_files: usize,
    processed_files: usize,
) {
    control.publish(SourceSyncProgress::from_report(
        SourceSyncPhase::Syncing,
        total_files,
        processed_files,
        None,
        report,
    ));
}

#[derive(Debug, Clone, Copy)]
struct SourceStamp {
    size_bytes: u64,
    mtime_ns: i64,
}

fn source_stamp(path: &Path, policy: &SourceScanPolicy) -> Result<SourceStamp, AppError> {
    let metadata = std::fs::metadata(path)?;
    let size_bytes = metadata.len();
    if let Some(limit) = policy.max_bytes() {
        if size_bytes > limit {
            return Err(AppError::config(format!(
                "source file exceeds max_file_bytes ({size_bytes} > {limit}): {}",
                path.display()
            )));
        }
    }
    let modified = metadata.modified()?;
    let elapsed = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    let mtime_ns = i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX);
    Ok(SourceStamp {
        size_bytes,
        mtime_ns,
    })
}

fn manifest_preflight_matches(
    entry: &SourceManifestEntry,
    stamp: SourceStamp,
    canonical_root: &str,
) -> bool {
    entry.manifest_present
        && entry.canonical_root == canonical_root
        && entry.size_bytes == stamp.size_bytes
        && entry.mtime_ns == stamp.mtime_ns
        && entry.document_exists
        && (entry.chunks_exist || !entry.document_indexable)
        && entry.document_content_hash.as_deref() == Some(entry.content_hash.as_str())
}

fn manifest_content_matches(entry: &SourceManifestEntry, content_hash: &str) -> bool {
    if !entry.document_exists || (entry.document_indexable && !entry.chunks_exist) {
        return false;
    }
    entry.document_content_hash.as_deref().or_else(|| {
        entry
            .manifest_present
            .then_some(entry.content_hash.as_str())
    }) == Some(content_hash)
}

fn requested_scope_matches(entry: &SourceManifestEntry, command: &SourceSyncCommand) -> bool {
    requested_scope(command.wing.as_deref())
        .is_none_or(|wing| entry.document_wing.as_deref() == Some(wing))
        && requested_scope(command.room.as_deref())
            .is_none_or(|room| entry.document_room.as_deref() == Some(room))
}

fn requested_scope(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn inferred_scope(root: &Path, path: &Path) -> (String, String) {
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut components = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str());
    if root_name.eq_ignore_ascii_case("sources") {
        (
            components.next().unwrap_or("project").to_string(),
            components.next().unwrap_or("root").to_string(),
        )
    } else {
        let room = relative
            .parent()
            .and_then(|parent| parent.components().next())
            .and_then(|part| part.as_os_str().to_str())
            .unwrap_or("root")
            .to_string();
        (root_name.to_string(), room)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::error::Result;

    struct CountingEmbedder {
        inner: MockEmbedder,
        batches: Arc<AtomicUsize>,
    }

    struct GatedEmbedder {
        inner: MockEmbedder,
        calls: AtomicUsize,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    struct RecordingEmbedder {
        dims: usize,
        calls: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct FailingBatchEmbedder {
        dims: usize,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EmbeddingProvider for CountingEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.batches.fetch_add(1, Ordering::Relaxed);
            self.inner.embed(texts).await
        }

        fn dimensions(&self) -> usize {
            self.inner.dimensions()
        }
    }

    #[async_trait]
    impl EmbeddingProvider for GatedEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.embed(texts).await
        }

        fn dimensions(&self) -> usize {
            self.inner.dimensions()
        }
    }

    #[async_trait]
    impl EmbeddingProvider for RecordingEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.lock().unwrap().push(texts.to_vec());
            Ok(texts
                .iter()
                .enumerate()
                .map(|(index, _)| vec![(index + 1) as f32; self.dims])
                .collect())
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FailingBatchEmbedder {
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(AppError::embeddings("injected batch embedding failure"))
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    #[test]
    fn only_explicit_not_found_is_classified_as_deleted() {
        assert!(metadata_means_missing(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "gone",
        )))
        .unwrap());
        let error = metadata_means_missing(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "temporarily unreadable",
        )))
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn permission_denied_source_is_reported_without_deleting_indexed_state() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            db_path: root.path().join("permission-denied.duckdb"),
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let source = "/vault/private.md";
        let document = crate::models::Document {
            id: "permission-doc".into(),
            uri: "file:///vault/private.md".into(),
            title: "Private".into(),
            content: "indexed content".into(),
            content_hash: Some(content_hash("indexed content")),
            source_file: Some(source.into()),
            layer: "raw".into(),
            kind: "document".into(),
            ..Default::default()
        };
        store.upsert_document(&document).unwrap();
        store
            .upsert_source_manifest(SourceManifestWrite {
                canonical_path: source,
                canonical_root: "/vault",
                size_bytes: 15,
                mtime_ns: 1,
                content_hash: document.content_hash.as_deref().unwrap(),
                document_id: &document.id,
            })
            .unwrap();
        let preloaded = store
            .load_source_manifest_root(Path::new("/vault"))
            .unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let mut report = SourceSyncReport::default();

        let cancelled = service
            .remove_deleted_with_metadata(
                &BTreeSet::new(),
                &preloaded,
                &mut report,
                &SourceSyncControl::default(),
                0,
                |_| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "temporarily unreadable",
                    ))
                },
            )
            .unwrap();

        assert!(!cancelled);
        assert!(report.deleted.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].error.contains("preserving indexed state"));
        assert!(store.get_document(&document.id).unwrap().is_some());
        assert!(store
            .load_source_manifest_root(Path::new("/vault"))
            .unwrap()
            .contains_key(source));
    }

    #[tokio::test]
    async fn store_clones_serialize_source_sync_runs() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "# Serialized").unwrap();
        let config = Arc::new(Config {
            db_path: db.path().join("serialized.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        });
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
            inner: MockEmbedder::new(16),
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
        });
        let command = SourceSyncCommand::new(root.path().to_path_buf());

        let first = {
            let store = store.clone();
            let config = config.clone();
            let embedder = embedder.clone();
            let command = command.clone();
            tokio::spawn(async move {
                SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                    .sync(command)
                    .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("first sync reached embedding");

        let second_scanning = Arc::new(tokio::sync::Notify::new());
        let (second_spawned_tx, second_spawned_rx) = tokio::sync::oneshot::channel();
        let second = {
            let store = store.clone();
            let config = config.clone();
            let embedder = embedder.clone();
            let scanning = second_scanning.clone();
            tokio::spawn(async move {
                let control = SourceSyncControl::new(CancellationToken::new(), move |progress| {
                    if progress.phase == SourceSyncPhase::Scanning {
                        scanning.notify_one();
                    }
                });
                let _ = second_spawned_tx.send(());
                SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                    .sync_with_control(command, control)
                    .await
            })
        };
        second_spawned_rx.await.unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(100),
            second_scanning.notified(),
        )
        .await
        .is_err());

        release.notify_one();
        first.await.unwrap().unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            second_scanning.notified(),
        )
        .await
        .expect("second sync entered after first released the lane");
        let outcome = second.await.unwrap().unwrap();
        assert!(matches!(outcome, SourceSyncOutcome::Completed(_)));
        assert_eq!(store.list_documents().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn small_documents_share_one_embedding_call_and_keep_path_mapping() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("c.md", "charlie body"),
            ("a.md", "alpha body"),
            ("b.md", "bravo body"),
        ] {
            std::fs::write(root.path().join(name), body).unwrap();
        }
        let config = Config {
            db_path: db.path().join("mapped-batch.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 1_024,
            chunk_overlap: 0,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
            dims: 4,
            calls: calls.clone(),
        });

        let report = SourceSyncService::new(&store, &embedder, &config)
            .sync(SourceSyncCommand::new(root.path().to_path_buf()))
            .await
            .unwrap();

        let canonical_paths =
            ["a.md", "b.md", "c.md"].map(|name| root.path().join(name).canonicalize().unwrap());
        assert_eq!(
            report.added,
            canonical_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
        );
        assert!(report.errors.is_empty());
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[vec![
                "alpha body".to_string(),
                "bravo body".to_string(),
                "charlie body".to_string(),
            ]]
        );
        for (index, path) in canonical_paths.iter().enumerate() {
            let document = store
                .find_by_uri(&format!("file://{}", path.display()))
                .unwrap()
                .unwrap();
            let chunks = store.list_chunks_for_document(&document.id).unwrap();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].embedding[0], (index + 1) as f32);
        }
    }

    #[tokio::test]
    async fn sixty_five_small_documents_split_at_the_batch_cap() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        for index in 0..65 {
            std::fs::write(
                root.path().join(format!("{index:03}.md")),
                format!("body {index}"),
            )
            .unwrap();
        }
        let config = Config {
            db_path: db.path().join("batch-cap.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 1_024,
            chunk_overlap: 0,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
            dims: 4,
            calls: calls.clone(),
        });

        let report = SourceSyncService::new(&store, &embedder, &config)
            .sync(SourceSyncCommand::new(root.path().to_path_buf()))
            .await
            .unwrap();

        assert!(report.errors.is_empty());
        assert_eq!(report.added.len(), 65);
        assert_eq!(store.list_documents().unwrap().len(), 65);
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![64, 1]
        );
    }

    #[tokio::test]
    async fn chunk_count_splits_a_batch_before_the_document_count_cap() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        for index in 0..33 {
            std::fs::write(root.path().join(format!("{index:03}.md")), "abcdefgh").unwrap();
        }
        let config = Config {
            db_path: db.path().join("chunk-batch-cap.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 4,
            chunk_overlap: 0,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
            dims: 4,
            calls: calls.clone(),
        });

        let report = SourceSyncService::new(&store, &embedder, &config)
            .sync(SourceSyncCommand::new(root.path().to_path_buf()))
            .await
            .unwrap();

        assert!(report.errors.is_empty());
        assert_eq!(report.added.len(), 33);
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![64, 2]
        );
    }

    #[tokio::test]
    async fn document_over_chunk_cap_is_embedded_alone_without_internal_splitting() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.md"), "aaaa").unwrap();
        std::fs::write(root.path().join("b.md"), "b".repeat(260)).unwrap();
        std::fs::write(root.path().join("c.md"), "cccc").unwrap();
        let config = Config {
            db_path: db.path().join("large-document-batch.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 4,
            chunk_overlap: 0,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
            dims: 4,
            calls: calls.clone(),
        });

        let report = SourceSyncService::new(&store, &embedder, &config)
            .sync(SourceSyncCommand::new(root.path().to_path_buf()))
            .await
            .unwrap();

        assert!(report.errors.is_empty());
        assert_eq!(report.added.len(), 3);
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![1, 65, 1]
        );
    }

    #[tokio::test]
    async fn embedding_failure_writes_none_of_a_multi_document_batch() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        for name in ["c.md", "a.md", "b.md"] {
            std::fs::write(root.path().join(name), format!("body {name}")).unwrap();
        }
        let config = Config {
            db_path: db.path().join("failed-batch.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 1_024,
            chunk_overlap: 0,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailingBatchEmbedder {
            dims: 4,
            calls: calls.clone(),
        });

        let report = SourceSyncService::new(&store, &embedder, &config)
            .sync(SourceSyncCommand::new(root.path().to_path_buf()))
            .await
            .unwrap();

        let expected_paths = ["a.md", "b.md", "c.md"]
            .map(|name| {
                root.path()
                    .join(name)
                    .canonicalize()
                    .unwrap()
                    .display()
                    .to_string()
            })
            .to_vec();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            report
                .errors
                .iter()
                .map(|error| error.path.clone())
                .collect::<Vec<_>>(),
            expected_paths
        );
        assert!(report
            .errors
            .iter()
            .all(|error| error.error.contains("injected batch embedding failure")));
        assert!(report.added.is_empty());
        assert_eq!(report.counters.embedded, 0);
        assert!(store.list_documents().unwrap().is_empty());
        assert!(store
            .load_source_manifest_root(root.path())
            .unwrap()
            .is_empty());
        assert!(store.list_graph_edges().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_during_embedding_writes_none_of_the_current_batch() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        for name in ["a.md", "b.md", "c.md"] {
            std::fs::write(root.path().join(name), format!("body {name}")).unwrap();
        }
        let config = Arc::new(Config {
            db_path: db.path().join("cancel-embedding.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 1_024,
            chunk_overlap: 0,
            ..Config::for_tests()
        });
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
            inner: MockEmbedder::new(4),
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release,
        });
        let cancellation = CancellationToken::new();
        let task = {
            let store = store.clone();
            let config = config.clone();
            let embedder = embedder.clone();
            let cancellation = cancellation.clone();
            let path = root.path().to_path_buf();
            tokio::spawn(async move {
                SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                    .sync_with_control(
                        SourceSyncCommand::new(path),
                        SourceSyncControl::new(cancellation, |_| {}),
                    )
                    .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("batch reached embedding");

        cancellation.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("cancelled sync returned")
            .unwrap()
            .unwrap();

        let SourceSyncOutcome::Cancelled(report) = outcome else {
            panic!("expected cancellation");
        };
        assert_eq!(report.counters.extracted, 3);
        assert!(report.added.is_empty());
        assert!(report.updated.is_empty());
        assert!(store.list_documents().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_between_commits_persists_exactly_the_sorted_prefix() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        for name in ["c.md", "a.md", "b.md"] {
            std::fs::write(root.path().join(name), format!("body {name}")).unwrap();
        }
        let config = Config {
            db_path: db.path().join("cancel-commits.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 1_024,
            chunk_overlap: 0,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
            dims: 4,
            calls: calls.clone(),
        });
        let cancellation = CancellationToken::new();
        let cancellation_from_progress = cancellation.clone();
        let progress = Arc::new(Mutex::new(Vec::new()));
        let progress_from_callback = progress.clone();
        let control = SourceSyncControl::new(cancellation, move |snapshot| {
            if snapshot.phase == SourceSyncPhase::Syncing
                && snapshot.current_path.is_none()
                && snapshot.added == 1
            {
                cancellation_from_progress.cancel();
            }
            progress_from_callback.lock().unwrap().push(snapshot);
        });

        let outcome = SourceSyncService::new(&store, &embedder, &config)
            .sync_with_control(SourceSyncCommand::new(root.path().to_path_buf()), control)
            .await
            .unwrap();

        let SourceSyncOutcome::Cancelled(report) = outcome else {
            panic!("expected cancellation");
        };
        let canonical_first = root
            .path()
            .join("a.md")
            .canonicalize()
            .unwrap()
            .display()
            .to_string();
        assert_eq!(report.added, vec![canonical_first]);
        assert_eq!(report.counters.embedded, 1);
        assert_eq!(store.list_documents().unwrap().len(), 1);
        assert_eq!(store.list_documents().unwrap()[0].title, "a.md");
        assert_eq!(calls.lock().unwrap().len(), 1);
        let progress = progress.lock().unwrap();
        let final_snapshot = progress.last().unwrap();
        assert_eq!(final_snapshot.phase, SourceSyncPhase::Cancelled);
        assert_eq!(final_snapshot.processed_files, 1);
    }

    #[tokio::test]
    async fn source_commit_keeps_the_revision_captured_before_embedding() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let source = root.path().join("a.md");
        std::fs::write(&source, "initial source").unwrap();
        let config = Arc::new(Config {
            db_path: db.path().join("source-cas.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 4,
            chunk_size: 1_024,
            chunk_overlap: 0,
            ..Config::for_tests()
        });
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let initial_embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(4));
        SourceSyncService::new(store.as_ref(), &initial_embedder, config.as_ref())
            .sync(SourceSyncCommand::new(root.path().to_path_buf()))
            .await
            .unwrap();

        std::fs::write(&source, "source sync contender").unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let gated_embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
            inner: MockEmbedder::new(4),
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
        });
        let task = {
            let store = store.clone();
            let config = config.clone();
            let embedder = gated_embedder.clone();
            let path = root.path().to_path_buf();
            tokio::spawn(async move {
                SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                    .sync(SourceSyncCommand::new(path))
                    .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("source update reached embedding");

        let canonical = source.canonicalize().unwrap();
        let existing = store
            .find_by_uri(&format!("file://{}", canonical.display()))
            .unwrap()
            .unwrap();
        IngestService::new(store.as_ref(), &initial_embedder, config.as_ref())
            .ingest(IngestCommand {
                text: "external winner".into(),
                title: Some("a.md".into()),
                uri: Some(existing.uri.clone()),
                metadata_json: Some(existing.metadata_json.clone()),
                wing: existing.wing.clone(),
                room: existing.room.clone(),
                source_file: existing.source_file.clone(),
                layer: existing.layer.clone(),
                kind: existing.kind.clone(),
                immutable: false,
            })
            .await
            .unwrap();
        release.notify_one();

        let report = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("source sync completed")
            .unwrap()
            .unwrap();
        let persisted = store.get_document(&existing.id).unwrap().unwrap();
        assert_eq!(persisted.content, "external winner");
        assert_eq!(persisted.revision, 2);
        assert!(report.added.is_empty());
        assert!(report.updated.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].error.contains("revision"));
    }

    #[tokio::test]
    async fn sync_preflight_updates_one_changed_file_and_cleans_deleted_manifest() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let project = root.path().join("alpha");
        std::fs::create_dir(&project).unwrap();
        let source = project.join("README.md");
        std::fs::write(&source, "# Alpha\nbody").unwrap();
        let canonical_project = project.canonicalize().unwrap();
        let canonical_source = source.canonicalize().unwrap().display().to_string();
        let config = Config {
            db_path: db.path().join("test.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embed_batches = Arc::new(AtomicUsize::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
            inner: MockEmbedder::new(16),
            batches: embed_batches.clone(),
        });
        let service = SourceSyncService::new(&store, &embedder, &config);
        let command = SourceSyncCommand {
            path: project.clone(),
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: Some("root".into()),
            max_file_bytes: None,
        };

        let first = service.sync(command.clone()).await.unwrap();
        assert!(first.errors.is_empty(), "first={first:?}");
        assert_eq!(first.added, vec![canonical_source.clone()]);
        assert_eq!(first.counters.preflight, 1);
        assert_eq!(first.counters.extracted, 1);
        assert_eq!(first.counters.embedded, 1);
        let batches_after_first = embed_batches.load(Ordering::Relaxed);
        assert!(batches_after_first > 0);

        let second = service.sync(command.clone()).await.unwrap();
        assert!(second.errors.is_empty(), "second={second:?}");
        assert_eq!(second.skipped, vec![canonical_source.clone()]);
        assert_eq!(second.counters.preflight, 1);
        assert_eq!(second.counters.extracted, 0);
        assert_eq!(second.counters.embedded, 0);
        assert_eq!(embed_batches.load(Ordering::Relaxed), batches_after_first);

        let document = store.list_documents().unwrap().pop().unwrap();
        assert_eq!(document.wing.as_deref(), Some("alpha"));
        assert_eq!(document.room.as_deref(), Some("root"));
        let roots = store.list_source_roots(Some("alpha")).unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].canonical_root,
            canonical_project.display().to_string()
        );
        assert_eq!(roots[0].file_count, 1);
        assert!(roots[0].last_seen.is_some());
        assert!(roots[0].last_synced.is_some());
        assert!(store.list_source_roots(Some("other")).unwrap().is_empty());

        let batches_before_repair = embed_batches.load(Ordering::Relaxed);
        store.delete_chunks_for_document(&document.id).unwrap();
        let repaired = service.sync(command.clone()).await.unwrap();
        assert!(repaired.errors.is_empty(), "repaired={repaired:?}");
        assert_eq!(repaired.updated, vec![canonical_source.clone()]);
        assert_eq!(repaired.counters.extracted, 1);
        assert_eq!(repaired.counters.embedded, 1);
        assert!(embed_batches.load(Ordering::Relaxed) > batches_before_repair);

        let warm_after_repair = service.sync(command.clone()).await.unwrap();
        assert_eq!(warm_after_repair.skipped, vec![canonical_source.clone()]);
        assert_eq!(warm_after_repair.counters.extracted, 0);
        assert_eq!(warm_after_repair.counters.embedded, 0);

        std::fs::write(&source, "# Alpha\nbody changed and longer").unwrap();
        let changed = service.sync(command.clone()).await.unwrap();
        assert!(changed.errors.is_empty(), "changed={changed:?}");
        assert_eq!(changed.updated, vec![canonical_source.clone()]);
        assert_eq!(changed.counters.preflight, 1);
        assert_eq!(changed.counters.extracted, 1);
        assert_eq!(changed.counters.embedded, 1);
        assert!(embed_batches.load(Ordering::Relaxed) > batches_after_first);

        std::fs::remove_file(&source).unwrap();
        let deleted = service
            .sync(SourceSyncCommand {
                remove_deleted: true,
                ..command
            })
            .await
            .unwrap();
        assert!(deleted.errors.is_empty(), "deleted={deleted:?}");
        assert_eq!(deleted.deleted, vec![canonical_source]);
        assert_eq!(deleted.counters.preflight, 0);
        assert_eq!(deleted.counters.extracted, 0);
        assert_eq!(deleted.counters.embedded, 0);
        assert!(store.list_documents().unwrap().is_empty());
        assert!(store
            .load_source_manifest_root(&canonical_project)
            .unwrap()
            .is_empty());
        assert!(store.list_source_roots(None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn controlled_sync_reports_progress_and_cancels_before_the_next_write() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.md"), "# A").unwrap();
        std::fs::write(root.path().join("b.md"), "# B").unwrap();
        let config = Config {
            db_path: db.path().join("test.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let cancellation = CancellationToken::new();
        let cancellation_from_progress = cancellation.clone();
        let phases = Arc::new(Mutex::new(Vec::new()));
        let phases_from_progress = phases.clone();
        let control = SourceSyncControl::new(cancellation, move |progress| {
            phases_from_progress.lock().unwrap().push(progress.phase);
            if progress.phase == SourceSyncPhase::Syncing && progress.current_path.is_some() {
                cancellation_from_progress.cancel();
            }
        });

        let outcome = SourceSyncService::new(&store, &embedder, &config)
            .sync_with_control(
                SourceSyncCommand {
                    path: root.path().to_path_buf(),
                    remove_deleted: false,
                    wing: None,
                    room: None,
                    max_file_bytes: None,
                },
                control,
            )
            .await
            .unwrap();

        let SourceSyncOutcome::Cancelled(report) = outcome else {
            panic!("expected a cancelled source sync");
        };
        assert!(report.added.is_empty());
        assert!(store.list_documents().unwrap().is_empty());
        let phases = phases.lock().unwrap();
        assert!(phases.contains(&SourceSyncPhase::Scanning));
        assert!(phases.contains(&SourceSyncPhase::Syncing));
        assert_eq!(phases.last(), Some(&SourceSyncPhase::Cancelled));
    }

    #[tokio::test]
    async fn warm_sync_reapplies_an_explicit_scope_change() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "# Scope").unwrap();
        let config = Config {
            db_path: db.path().join("scope.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let mut command = SourceSyncCommand {
            path: root.path().to_path_buf(),
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: Some("root".into()),
            max_file_bytes: None,
        };
        service.sync(command.clone()).await.unwrap();

        command.wing = Some("  beta  ".into());
        let changed = service.sync(command).await.unwrap();

        assert_eq!(changed.updated.len(), 1);
        assert_eq!(changed.counters.extracted, 1);
        assert_eq!(changed.counters.embedded, 1);
        let document = store.list_documents().unwrap().pop().unwrap();
        assert_eq!(document.wing.as_deref(), Some("beta"));
        assert_eq!(document.room.as_deref(), Some("root"));
    }

    #[tokio::test]
    async fn empty_source_is_a_stable_non_searchable_manifest_entry() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("empty.md"), "  \n").unwrap();
        let config = Config {
            db_path: db.path().join("empty.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let command = SourceSyncCommand {
            path: root.path().to_path_buf(),
            remove_deleted: false,
            wing: Some("empty".into()),
            room: None,
            max_file_bytes: None,
        };

        let first = service.sync(command.clone()).await.unwrap();
        assert_eq!(first.added.len(), 1);
        assert_eq!(store.integrity_counts().unwrap().0, 0);
        let second = service.sync(command).await.unwrap();
        assert_eq!(second.skipped.len(), 1);
        assert_eq!(second.counters.extracted, 0);
        assert_eq!(second.counters.embedded, 0);
    }

    #[tokio::test]
    async fn manifest_moves_between_parent_and_child_roots_without_losing_cleanup() {
        let sources = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let project = sources.path().join("alpha");
        std::fs::create_dir(&project).unwrap();
        let source = project.join("README.md");
        std::fs::write(&source, "# Alpha").unwrap();
        let config = Config {
            db_path: db.path().join("roots.duckdb"),
            ingest_roots: vec![sources.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        service
            .sync(SourceSyncCommand {
                path: sources.path().to_path_buf(),
                remove_deleted: false,
                wing: Some("alpha".into()),
                room: None,
                max_file_bytes: None,
            })
            .await
            .unwrap();

        let moved = service
            .sync(SourceSyncCommand {
                path: project.clone(),
                remove_deleted: false,
                wing: Some("alpha".into()),
                room: None,
                max_file_bytes: None,
            })
            .await
            .unwrap();
        assert_eq!(moved.skipped.len(), 1);
        assert_eq!(moved.counters.extracted, 1);
        assert_eq!(moved.counters.embedded, 0);
        let canonical_project = project.canonicalize().unwrap();
        let manifest = store.load_source_manifest_root(&canonical_project).unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(
            manifest.values().next().unwrap().canonical_root,
            canonical_project.display().to_string()
        );

        let canonical_parent = sources.path().canonicalize().unwrap();
        let moved_to_parent = service
            .sync(SourceSyncCommand {
                path: sources.path().to_path_buf(),
                remove_deleted: false,
                wing: Some("alpha".into()),
                room: None,
                max_file_bytes: None,
            })
            .await
            .unwrap();
        assert_eq!(moved_to_parent.skipped.len(), 1);
        assert_eq!(moved_to_parent.counters.extracted, 1);
        assert_eq!(moved_to_parent.counters.embedded, 0);
        let manifest = store.load_source_manifest_root(&canonical_parent).unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(
            manifest.values().next().unwrap().canonical_root,
            canonical_parent.display().to_string()
        );

        let moved_back_to_child = service
            .sync(SourceSyncCommand {
                path: project.clone(),
                remove_deleted: false,
                wing: Some("alpha".into()),
                room: None,
                max_file_bytes: None,
            })
            .await
            .unwrap();
        assert_eq!(moved_back_to_child.skipped.len(), 1);
        assert_eq!(moved_back_to_child.counters.extracted, 1);
        assert_eq!(moved_back_to_child.counters.embedded, 0);

        std::fs::remove_file(source).unwrap();
        let removed = service
            .sync(SourceSyncCommand {
                path: project,
                remove_deleted: true,
                wing: Some("alpha".into()),
                room: None,
                max_file_bytes: None,
            })
            .await
            .unwrap();
        assert_eq!(removed.deleted.len(), 1);
        assert!(store.list_documents().unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_sources_are_reported_and_never_silently_stale() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let source = root.path().join("large.md");
        std::fs::write(&source, "indexed body").unwrap();
        let config = Config {
            db_path: db.path().join("oversize.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let mut command = SourceSyncCommand {
            path: root.path().to_path_buf(),
            remove_deleted: false,
            wing: Some("large".into()),
            room: None,
            max_file_bytes: Some(100),
        };
        service.sync(command.clone()).await.unwrap();
        let original = store.list_documents().unwrap().pop().unwrap().content;

        std::fs::write(&source, "now much larger than the configured cap").unwrap();
        command.max_file_bytes = Some(3);
        let report = service.sync(command).await.unwrap();
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].error.contains("exceeds max_file_bytes"));
        assert!(report.updated.is_empty());
        assert_eq!(
            store.list_documents().unwrap().pop().unwrap().content,
            original
        );
    }

    #[test]
    fn source_stamp_rejects_a_file_that_exceeds_the_scan_guard() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("large.md");
        std::fs::write(&source, "too large").unwrap();
        let policy = SourceScanPolicy::default().with_max_bytes(3);

        let error = source_stamp(&source, &policy).unwrap_err();
        assert!(error.to_string().contains("exceeds max_file_bytes"));
    }
}
