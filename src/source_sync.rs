//! Application service for synchronizing a filesystem tree into the corpus.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use rag_mcp_core::{
    SourceSyncCounters, SourceSyncError, SourceSyncFinalizationError, SourceSyncPhase,
    SourceSyncProgress,
};

use crate::config::Config;
use crate::db::{SourceManifestEntry, SourceManifestWrite, Store};
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::file_ingest::{extract_file, is_supported_source, merge_metadata};
use crate::ingest::{IngestCommand, IngestService, PreparedIngest, SourceManifestStamp};
use crate::source_scan::{
    collect_source_files_while, is_explicitly_excluded_source_path, SourceScanPolicy,
};
use crate::util::{check_path_allowlist, content_hash};

/// Default safety cap for one file in recursive source synchronization.
pub const DEFAULT_SOURCE_SYNC_MAX_FILE_BYTES: u64 = 512 * 1024;

const SOURCE_SYNC_BATCH_MAX_DOCUMENTS: usize = 64;
const SOURCE_SYNC_BATCH_MAX_CHUNKS: usize = 64;
const FTS_FINALIZATION_ERROR_CODE: &str = "FTS_FINALIZATION_FAILED";


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



#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceSyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
    pub deleted: Vec<String>,
    pub errors: Vec<SourceSyncError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalization_error: Option<SourceSyncFinalizationError>,
    pub counters: SourceSyncCounters,
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



fn progress_from_report(
    phase: SourceSyncPhase,
    total_files: usize,
    processed_files: usize,
    current_path: Option<String>,
    report: &SourceSyncReport,
) -> SourceSyncProgress {
    SourceSyncProgress {
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

/// Drive source synchronization from Tokio's blocking pool.
///
/// The workflow intentionally mixes filesystem traversal/extraction and
/// serialized DuckDB work with async embedding requests. Polling it on a
/// blocking worker keeps those synchronous phases off Tokio's async workers;
/// the runtime handle still services the provider awaits and cancellation.
pub async fn sync_sources_nonblocking(
    store: Arc<Store>,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Config,
    command: SourceSyncCommand,
    control: SourceSyncControl,
) -> Result<SourceSyncOutcome, AppError> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(
            SourceSyncService::new(&store, &embedder, &config).sync_with_control(command, control),
        )
    })
    .await
    .map_err(|error| AppError::db(format!("source-sync task failed: {error}")))?
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
        let sync_lane = self.store.corpus_mutation_lane();
        let sync_guard = tokio::select! {
            guard = sync_lane.write_owned() => guard,
            () = control.cancelled() => {
                return Ok(cancelled_outcome(&control, report, 0, 0));
            }
        };
        control.publish(progress_from_report(
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
        // Every early exit below reports the same cancelled outcome, and every
        // batch flush either completes (resetting the chunk budget) or cancels.
        macro_rules! cancelled {
            ($processed:expr) => {
                return Ok(cancelled_outcome(&control, report, total_files, $processed))
            };
        }
        macro_rules! flush_pending {
            () => {
                match self
                    .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                    .await
                {
                    BatchFlushOutcome::Completed(processed) => {
                        pending_chunks = 0;
                        processed
                    }
                    BatchFlushOutcome::Cancelled(processed) => cancelled!(processed),
                }
            };
        }
        for (index, path) in files.into_iter().enumerate() {
            control.publish(progress_from_report(
                SourceSyncPhase::Syncing,
                total_files,
                processed_files,
                Some(path.display().to_string()),
                &report,
            ));
            if control.is_cancelled() {
                cancelled!(processed_files);
            }
            report.counters.preflight = report.counters.preflight.saturating_add(1);

            let preparation = self.prepare_source_file(&run, &path, &mut seen, &mut report);
            if control.is_cancelled() {
                cancelled!(processed_files);
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
                        processed_files = flush_pending!();
                        if control.is_cancelled() {
                            cancelled!(processed_files);
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
                        processed_files = flush_pending!();
                    }
                }
                Ok(PreparedSourceFile::Skipped {
                    source_file,
                    manifest_refresh,
                }) => {
                    if !pending.is_empty() {
                        let processed = flush_pending!();
                        if control.is_cancelled() {
                            cancelled!(processed);
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
                        flush_pending!();
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
                cancelled!(processed_files);
            }
        }
        if !pending.is_empty() {
            match self
                .flush_pending_batch(&mut pending, &control, &mut report, total_files)
                .await
            {
                BatchFlushOutcome::Completed(processed) => processed_files = processed,
                BatchFlushOutcome::Cancelled(processed) => cancelled!(processed),
            }
        }
        if control.is_cancelled() {
            cancelled!(processed_files);
        }
        if command.remove_deleted {
            control.publish(progress_from_report(
                SourceSyncPhase::RemovingDeleted,
                total_files,
                total_files,
                None,
                &report,
            ));
            if control.is_cancelled() {
                cancelled!(total_files);
            }
            if self.remove_deleted_with_probe(
                &seen,
                &preloaded,
                &mut report,
                &control,
                total_files,
                |path| {
                    if is_explicitly_excluded_source_path(&root, path) {
                        Ok(true)
                    } else {
                        metadata_means_missing(std::fs::metadata(path))
                    }
                },
            )? {
                cancelled!(total_files);
            }
        }
        if control.is_cancelled() {
            cancelled!(total_files);
        }
        let seen_paths = seen.iter().cloned().collect::<Vec<_>>();
        self.store.mark_source_manifest_seen(&root, &seen_paths)?;
        control.publish(progress_from_report(
            SourceSyncPhase::RefreshingFts,
            total_files,
            total_files,
            None,
            &report,
        ));
        if control.is_cancelled() {
            cancelled!(total_files);
        }
        // A source sync owns the lexical coordination lane until its derived
        // index is generation-clean. Otherwise the first user search after a
        // bulk update pays the entire serialized BM25 rebuild cost.
        let store = self.store.clone();
        let stemmer = self.config.fts_stemmer.clone();
        let durable_mutation_committed =
            !report.added.is_empty() || !report.updated.is_empty() || !report.deleted.is_empty();
        let refresh_task = tokio::task::spawn_blocking(move || {
            let refresh_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finalize_source_sync_fts(&store, &stemmer)
            }))
            .unwrap_or_else(|_| Err(AppError::fts("source-sync FTS finalization panicked")));
            let finalization_error = refresh_result.err().map(|error| {
                source_sync_fts_finalization_failure(&store, durable_mutation_committed, &error)
            });
            (sync_guard, finalization_error)
        })
        .await;
        let (sync_guard, finalization_error) = match refresh_task {
            Ok((guard, None)) => (Some(guard), None),
            Ok((guard, Some(error))) => (Some(guard), Some(error)),
            Err(error) => {
                let error = AppError::fts(format!("source-sync FTS refresh task failed: {error}"));
                (
                    None,
                    Some(source_sync_fts_finalization_failure(
                        self.store,
                        durable_mutation_committed,
                        &error,
                    )),
                )
            }
        };
        if let Some(finalization_error) = finalization_error {
            report.errors.push(SourceSyncError {
                path: root.display().to_string(),
                error: source_sync_finalization_summary(&finalization_error),
            });
            report.finalization_error = Some(finalization_error);
        }
        // Retain the returned guard through terminal progress publication. If
        // this future is dropped while waiting, detached blocking work owns it.
        let _sync_guard = sync_guard;
        report.sort_paths();
        control.publish(progress_from_report(
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

        let mut metadata_json = merge_metadata(
            existing.and_then(|entry| entry.document_metadata_json.clone()),
            extracted.metadata,
        )
        .map_err(|error| AppError::config(error.to_string()))?;
        let (inferred_wing, inferred_room) = inferred_scope(run.root, &canonical);
        if belongs_to_dodo_group(run.root, &canonical) {
            metadata_json = add_metadata_tag(&metadata_json, "Dodo")?;
        }
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

    fn remove_deleted_with_probe(
        &self,
        seen: &BTreeSet<String>,
        preloaded: &HashMap<String, SourceManifestEntry>,
        report: &mut SourceSyncReport,
        control: &SourceSyncControl,
        total_files: usize,
        should_remove: impl Fn(&Path) -> std::io::Result<bool>,
    ) -> Result<bool, AppError> {
        let mut sources = preloaded.keys().collect::<Vec<_>>();
        sources.sort();
        for source in sources {
            control.publish(progress_from_report(
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
            let should_remove = match should_remove(path) {
                Ok(should_remove) => should_remove,
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
            if should_remove {
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

fn finalize_source_sync_fts(store: &Store, stemmer: &str) -> Result<(), AppError> {
    #[cfg(test)]
    crate::db::fts::test_fault::check("source-sync", stemmer)?;

    store.ensure_fts(stemmer).map(|_| ())
}

fn source_sync_fts_finalization_failure(
    store: &Store,
    durable_mutation_committed: bool,
    error: &AppError,
) -> SourceSyncFinalizationError {
    let recorded = store.record_fts_finalization_failure(
        "source_sync",
        durable_mutation_committed,
        &error.to_string(),
    );
    SourceSyncFinalizationError {
        code: FTS_FINALIZATION_ERROR_CODE.into(),
        stage: "refresh_fts".into(),
        durable_mutation_committed,
        retryable: true,
        fallback_dirty_marked: recorded.dirty_marker_written,
        dirty_marker_error: recorded.dirty_marker_error,
        ops_log_error: recorded.ops_log_error,
        error: error.to_string(),
    }
}

fn source_sync_finalization_summary(error: &SourceSyncFinalizationError) -> String {
    let mut summary = format!(
        "{}: source sync final FTS refresh failed after durable_mutation_committed={}: {}; retryable={}; fallback_dirty_marked={}",
        error.code,
        error.durable_mutation_committed,
        error.error,
        error.retryable,
        error.fallback_dirty_marked,
    );
    if let Some(marker_error) = &error.dirty_marker_error {
        summary.push_str(&format!("; dirty_marker_error={marker_error}"));
    }
    if let Some(log_error) = &error.ops_log_error {
        summary.push_str(&format!("; ops_log_error={log_error}"));
    }
    summary
}

fn cancelled_outcome(
    control: &SourceSyncControl,
    mut report: SourceSyncReport,
    total_files: usize,
    processed_files: usize,
) -> SourceSyncOutcome {
    report.sort_paths();
    control.publish(progress_from_report(
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
    control.publish(progress_from_report(
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
        let top_level = components.next().unwrap_or("project");
        if top_level.eq_ignore_ascii_case("dodo") {
            return (
                components.next().unwrap_or("Dodo").to_string(),
                components.next().unwrap_or("root").to_string(),
            );
        }
        (top_level.to_string(), components.next().unwrap_or("root").to_string())
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

fn belongs_to_dodo_group(root: &Path, path: &Path) -> bool {
    root.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("sources"))
        && path
            .strip_prefix(root)
            .ok()
            .and_then(|relative| relative.components().next())
            .and_then(|part| part.as_os_str().to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("dodo"))
}

fn add_metadata_tag(metadata_json: &str, tag: &str) -> Result<String, AppError> {
    let mut metadata: serde_json::Value = serde_json::from_str(metadata_json)
        .map_err(|error| AppError::config(error.to_string()))?;
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| AppError::config("document metadata must be a JSON object"))?;
    let tags = object
        .entry("tags")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let tags = tags
        .as_array_mut()
        .ok_or_else(|| AppError::config("document metadata tags must be an array"))?;
    if !tags.iter().any(|value| value.as_str() == Some(tag)) {
        tags.push(serde_json::Value::String(tag.to_string()));
    }
    serde_json::to_string(&metadata).map_err(|error| AppError::config(error.to_string()))
}

#[cfg(test)]
mod tests;
