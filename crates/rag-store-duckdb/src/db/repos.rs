//! Typed repository views over [`Store`].
//!
//! Each repository borrows [`Store`] (`Copy` + zero allocation) and forwards
//! to the underlying `lin`-backed [`Store`] methods, matching the multi-crate
//! repository surface introduced in Stage 8.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::EmbeddingIdentity;
use super::catalog::{DocumentCatalogFilter, DocumentCatalogPage, ProjectHome};
use super::fts::{FtsGenerationState, FtsState};
use super::kg::{KgFactInput, KgSupersedeInput};
use super::recovery::{
    BackupReport, BundleImportReport, ConflictPolicy, PortableBundlePreflight, RecoveryBundle,
};
use super::source_dedupe::SourceDuplicateCleanupReport;
use super::source_manifest::{SourceManifestEntry, SourceManifestWrite, SourceRootSummary};
use super::store::{
    AtomicDocumentWriteResult, CorpusMutationGuard, DocumentDerivedWrite, DocumentEmbeddingSample,
    DocumentRef, EmbeddingWriteGuard, ExactDuplicateMergeResult, FtsFinalizationFailure,
    LayerHealthCounts, Store, UncompiledRawSample, UncompiledShelfQueue, WikiDocumentWrite,
    WikiPageMetaFilter,
};
use super::sync::{SyncChange, SyncChangeInput, SyncIdentity, SyncStatus, WikiSyncWrite};
use super::vault::VaultExportReport;
use crate::error::Result;
use crate::models::{
    Chunk, Collection, CollectionDetail, CollectionEntry, DiaryEntry, Document, DocumentFilter,
    DocumentMetaApplyResult, DocumentMetaUpdate, DocumentRevisionPage, DuplicateCheckResult,
    EmbeddingManifest, GraphEdge, GraphFilter, GraphNode, GraphStats, GraphView, IndexQueryMatch,
    KgFact, KgStats, OpsLogEntry, PlacementUpdate, ProjectSummary, RoomCount, Taxonomy,
    VacuumStoreReport, WikiIndexEntry, WikiPageListItem, WingCount,
};

impl Store {
    /// The [`Documents`] repository.
    pub fn documents(&self) -> Documents<'_> {
        Documents { store: self }
    }

    /// The [`Chunks`] repository.
    pub fn chunks(&self) -> Chunks<'_> {
        Chunks { store: self }
    }

    /// The [`GraphRepo`] repository.
    pub fn graph(&self) -> GraphRepo<'_> {
        GraphRepo { store: self }
    }

    /// The [`KgRepo`] repository.
    pub fn kg(&self) -> KgRepo<'_> {
        KgRepo { store: self }
    }

    /// The [`OpsLog`] repository.
    pub fn ops_log(&self) -> OpsLog<'_> {
        OpsLog { store: self }
    }

    /// The [`EmbeddingManifestRepo`] repository.
    pub fn embedding_manifest(&self) -> EmbeddingManifestRepo<'_> {
        EmbeddingManifestRepo { store: self }
    }

    /// The [`WikiIndexRepo`] repository.
    pub fn wiki_index(&self) -> WikiIndexRepo<'_> {
        WikiIndexRepo { store: self }
    }

    /// The [`SyncRepo`] repository.
    pub fn sync(&self) -> SyncRepo<'_> {
        SyncRepo { store: self }
    }

    /// The [`CorpusLane`] repository.
    pub fn corpus_lane(&self) -> CorpusLane<'_> {
        CorpusLane { store: self }
    }

    /// The [`Collections`] repository.
    pub fn collections(&self) -> Collections<'_> {
        Collections { store: self }
    }

    /// The [`ReembedProgress`] repository.
    pub fn reembed_progress(&self) -> ReembedProgress<'_> {
        ReembedProgress { store: self }
    }

    /// The [`Maintenance`] repository.
    pub fn maintenance(&self) -> Maintenance<'_> {
        Maintenance { store: self }
    }

    /// The [`SourceManifestRepo`] repository.
    pub fn source_manifest(&self) -> SourceManifestRepo<'_> {
        SourceManifestRepo { store: self }
    }

    /// The [`Recovery`] repository.
    pub fn recovery(&self) -> Recovery<'_> {
        Recovery { store: self }
    }
}

#[derive(Clone, Copy)]
pub struct Documents<'a> {
    pub(in crate::db) store: &'a Store,
}

impl Documents<'_> {
    pub fn get_document(&self, id: &str) -> Result<Option<Document>> {
        self.store.get_document(id)
    }
    pub fn find_by_uri(&self, uri: &str) -> Result<Option<Document>> {
        self.store.find_by_uri(uri)
    }
    pub fn find_by_id_or_uri(&self, id: &str, uri: &str) -> Result<Option<Document>> {
        self.store.find_by_id_or_uri(id, uri)
    }
    pub fn content_hashes(&self, ids: &[&str]) -> Result<HashMap<String, Option<String>>> {
        self.store.content_hashes(ids)
    }
    pub fn find_by_content_hash(&self, hash: &str) -> Result<Option<Document>> {
        self.store.find_by_content_hash(hash)
    }
    pub fn list_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        self.store.list_by_content_hash(hash)
    }
    pub fn check_duplicate(
        &self,
        content: Option<&str>,
        content_hash_in: Option<&str>,
        uri: Option<&str>,
    ) -> Result<DuplicateCheckResult> {
        self.store.check_duplicate(content, content_hash_in, uri)
    }
    pub fn list_documents(&self) -> Result<Vec<Document>> {
        self.store.list_documents()
    }
    pub fn document_revision(&self, id: &str) -> Result<Option<i64>> {
        self.store.document_revision(id)
    }
    pub fn document_id_and_uri(&self, id: &str) -> Result<Option<(String, String)>> {
        self.store.document_id_and_uri(id)
    }
    pub fn list_document_refs(&self) -> Result<Vec<DocumentRef>> {
        self.store.list_document_refs()
    }
    pub fn count_documents(&self) -> Result<usize> {
        self.store.count_documents()
    }
    pub fn count_documents_by_layer(&self, layer: &str) -> Result<usize> {
        self.store.count_documents_by_layer(layer)
    }
    pub fn count_graph_rebuild_candidates(&self, dirty_only: bool) -> Result<usize> {
        self.store.count_graph_rebuild_candidates(dirty_only)
    }
    pub fn list_graph_rebuild_candidates_after(
        &self,
        dirty_only: bool,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Document>> {
        self.store
            .list_graph_rebuild_candidates_after(dirty_only, after_id, limit)
    }
    pub fn list_document_ids_after(
        &self,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.store.list_document_ids_after(after_id, limit)
    }
    pub fn count_chunks_for_document_limit(&self, limit: usize) -> Result<usize> {
        self.store.count_chunks_for_document_limit(limit)
    }
    pub fn exact_duplicate_source_ids(
        &self,
        hash: &str,
        keep_id: &str,
        limit: usize,
    ) -> Result<(usize, Vec<String>)> {
        self.store.exact_duplicate_source_ids(hash, keep_id, limit)
    }
    pub fn list_active_unscoped_documents(&self, limit: usize) -> Result<(usize, Vec<Document>)> {
        self.store.list_active_unscoped_documents(limit)
    }
    pub fn list_documents_missing_chunks(&self, limit: usize) -> Result<(usize, Vec<Document>)> {
        self.store.list_documents_missing_chunks(limit)
    }
    pub fn list_documents_by_layer(&self, layer: &str) -> Result<Vec<Document>> {
        self.store.list_documents_by_layer(layer)
    }
    pub fn list_documents_by_layer_without_body(&self, layer: &str) -> Result<Vec<Document>> {
        self.store.list_documents_by_layer_without_body(layer)
    }
    pub fn document_contents(&self, ids: &[&str]) -> Result<HashMap<String, String>> {
        self.store.document_contents(ids)
    }
    pub fn list_wiki_page_metas(&self) -> Result<Vec<WikiPageListItem>> {
        self.store.list_wiki_page_metas()
    }
    pub fn list_wiki_page_metas_filtered(
        &self,
        filter: &WikiPageMetaFilter,
    ) -> Result<(Vec<WikiPageListItem>, usize)> {
        self.store.list_wiki_page_metas_filtered(filter)
    }
    pub fn wiki_backlinks_for_document(&self, document_id: &str) -> Result<Vec<(String, String)>> {
        self.store.wiki_backlinks_for_document(document_id)
    }
    pub fn wiki_backlink_view(&self, document_id: &str) -> Result<Option<(String, GraphView)>> {
        self.store.wiki_backlink_view(document_id)
    }
    pub fn wiki_backlinks_for_document_in_wing(
        &self,
        document_id: &str,
        wing: &str,
    ) -> Result<Vec<(String, String)>> {
        self.store
            .wiki_backlinks_for_document_in_wing(document_id, wing)
    }
    pub fn list_documents_by_wing(&self, wing: &str) -> Result<Vec<Document>> {
        self.store.list_documents_by_wing(wing)
    }
    pub fn list_documents_filtered(&self, filter: &DocumentFilter) -> Result<Vec<Document>> {
        self.store.list_documents_filtered(filter)
    }
    pub fn list_wings(&self) -> Result<Vec<WingCount>> {
        self.store.list_wings()
    }
    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        self.store.list_projects()
    }
    pub fn list_rooms(&self, wing: Option<&str>) -> Result<Vec<RoomCount>> {
        self.store.list_rooms(wing)
    }
    pub fn get_taxonomy(&self) -> Result<Taxonomy> {
        self.store.get_taxonomy()
    }
    pub fn update_document_meta(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.store.update_document_meta(id, update)
    }
    pub fn update_document_meta_for_sync_test(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: &SyncIdentity,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.store
            .update_document_meta_for_sync_test(id, update, identity)
    }
    pub fn prepare_document_meta_update(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.store.prepare_document_meta_update(id, update)
    }
    pub fn update_document_placement(
        &self,
        id: &str,
        update: &PlacementUpdate,
    ) -> Result<Option<Document>> {
        self.store.update_document_placement(id, update)
    }
    pub fn upsert_document(&self, doc: &Document) -> Result<()> {
        self.store.upsert_document(doc)
    }
    pub fn upsert_document_cas(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
    ) -> Result<i64> {
        self.store.upsert_document_cas(doc, if_match_revision)
    }
    pub fn write_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.store
            .write_document_atomic(doc, if_match_revision, derived)
    }
    pub fn write_unchunked_document_with_audit(
        &self,
        doc: &Document,
        audit: &OpsLogEntry,
    ) -> Result<AtomicDocumentWriteResult> {
        self.store.write_unchunked_document_with_audit(doc, audit)
    }
    pub fn write_source_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        chunks: &[Chunk],
        manifest: SourceManifestWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.store
            .write_source_document_atomic(doc, if_match_revision, chunks, manifest)
    }
    pub fn write_wiki_document_atomic(
        &self,
        write: WikiDocumentWrite<'_>,
    ) -> Result<(Option<AtomicDocumentWriteResult>, i64)> {
        self.store.write_wiki_document_atomic(write)
    }
    pub fn delete_document(&self, id: &str) -> Result<bool> {
        self.store.delete_document(id)
    }
    pub fn apply_document_dispositions_atomic(
        &self,
        delete_ids: &[String],
        tombstone_ids: &[String],
    ) -> Result<(u64, u64, u64)> {
        self.store
            .apply_document_dispositions_atomic(delete_ids, tombstone_ids)
    }
    pub fn merge_exact_duplicates_atomic(
        &self,
        keep_id: &str,
        source_ids: &[String],
        declared_hash: Option<&str>,
        hard_delete: bool,
        allow_raw_delete: bool,
    ) -> Result<ExactDuplicateMergeResult> {
        self.store.merge_exact_duplicates_atomic(
            keep_id,
            source_ids,
            declared_hash,
            hard_delete,
            allow_raw_delete,
        )
    }
    pub fn delete_by_source(&self, source: &str) -> Result<u64> {
        self.store.delete_by_source(source)
    }
    pub fn delete_by_source_ids(&self, source: &str) -> Result<Vec<String>> {
        self.store.delete_by_source_ids(source)
    }
    pub fn delete_source_state(&self, source: &str) -> Result<(u64, bool)> {
        self.store.delete_source_state(source)
    }
    pub fn list_document_revisions(&self, document_id: &str) -> Result<Vec<Document>> {
        self.store.list_document_revisions(document_id)
    }
    pub fn list_document_revision_summaries(
        &self,
        document_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<DocumentRevisionPage> {
        self.store
            .list_document_revision_summaries(document_id, limit, offset)
    }
    pub fn get_document_revision(
        &self,
        document_id: &str,
        revision: i64,
    ) -> Result<Option<Document>> {
        self.store.get_document_revision(document_id, revision)
    }
    pub fn get_schema_document(&self) -> Result<Option<Document>> {
        self.store.get_schema_document()
    }
    pub fn diary_wing_for_agent(agent_name: &str) -> String {
        Store::diary_wing_for_agent(agent_name)
    }
    pub fn list_diary_entries(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        self.store.list_diary_entries(agent_name, limit)
    }
    pub fn list_diary_activity(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        self.store.list_diary_activity(agent_name, limit)
    }
    pub fn list_pinned_documents(&self, limit: usize) -> Result<Vec<Document>> {
        self.store.list_pinned_documents(limit)
    }
    pub fn list_document_catalog(
        &self,
        filter: &DocumentCatalogFilter,
    ) -> Result<DocumentCatalogPage> {
        self.store.list_document_catalog(filter)
    }
    pub fn project_home(&self, project_id: &str) -> Result<ProjectHome> {
        self.store.project_home(project_id)
    }
}

#[derive(Clone, Copy)]
pub struct Chunks<'a> {
    pub(in crate::db) store: &'a Store,
}

impl Chunks<'_> {
    pub fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>> {
        self.store.list_chunks_for_document(doc_id)
    }
    pub fn chunk_vectors_by_content(
        &self,
        document_ids: &[&str],
    ) -> Result<HashMap<String, Vec<f32>>> {
        self.store.chunk_vectors_by_content(document_ids)
    }
    pub fn representative_document_embeddings(
        &self,
        limit: usize,
        active_only: bool,
        require_wing: bool,
        exclude_system_layers: bool,
    ) -> Result<Vec<DocumentEmbeddingSample>> {
        self.store.representative_document_embeddings(
            limit,
            active_only,
            require_wing,
            exclude_system_layers,
        )
    }
    pub fn first_chunk_embeddings(
        &self,
        document_ids: &[&str],
    ) -> Result<BTreeMap<String, Vec<f32>>> {
        self.store.first_chunk_embeddings(document_ids)
    }
    pub fn all_chunks_with_embeddings(&self) -> Result<Vec<Chunk>> {
        self.store.all_chunks_with_embeddings()
    }
    pub fn replace_chunks_for_document(&self, document_id: &str, chunks: &[Chunk]) -> Result<()> {
        self.store.replace_chunks_for_document(document_id, chunks)
    }
    pub fn update_chunk_embeddings_atomic(
        &self,
        document_id: &str,
        if_match_revision: i64,
        expected_chunks: &[Chunk],
        embeddings: &[Vec<f32>],
        manifest: Option<&EmbeddingManifest>,
    ) -> Result<usize> {
        self.store.update_chunk_embeddings_atomic(
            document_id,
            if_match_revision,
            expected_chunks,
            embeddings,
            manifest,
        )
    }
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        self.store.insert_chunks(chunks)
    }
    pub fn count_chunks_for_document(&self, document_id: &str) -> Result<u64> {
        self.store.count_chunks_for_document(document_id)
    }
    pub fn delete_chunks_for_document(&self, document_id: &str) -> Result<()> {
        self.store.delete_chunks_for_document(document_id)
    }
}

#[derive(Clone, Copy)]
pub struct CorpusLane<'a> {
    pub(in crate::db) store: &'a Store,
}

impl CorpusLane<'_> {
    pub fn corpus_mutation_lane(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.store.corpus_mutation_lane()
    }
    pub fn try_corpus_mutation_guard(&self, operation: &str) -> Result<CorpusMutationGuard> {
        self.store.try_corpus_mutation_guard(operation)
    }
    pub fn with_corpus_mutation_guard(&self, guard: CorpusMutationGuard) -> Result<Store> {
        self.store.with_corpus_mutation_guard(guard)
    }
    pub fn try_corpus_mutation_scope(&self, operation: &str) -> Result<Store> {
        self.store.try_corpus_mutation_scope(operation)
    }
    pub fn try_embedding_write_guard(
        &self,
        config: &impl EmbeddingIdentity,
        operation: &str,
    ) -> Result<EmbeddingWriteGuard> {
        self.store.try_embedding_write_guard(config, operation)
    }
    pub fn record_fts_finalization_failure(
        &self,
        operation: &str,
        durable_mutation_committed: bool,
        error: &str,
    ) -> FtsFinalizationFailure {
        self.store
            .record_fts_finalization_failure(operation, durable_mutation_committed, error)
    }
}

#[derive(Clone, Copy)]
pub struct Maintenance<'a> {
    pub(in crate::db) store: &'a Store,
}

impl Maintenance<'_> {
    pub fn stats(&self) -> Result<(u64, u64, u64, u64)> {
        self.store.stats()
    }
    pub fn layer_health_counts(&self) -> Result<LayerHealthCounts> {
        self.store.layer_health_counts()
    }
    pub fn list_uncompiled_raw_sample(&self, limit: usize) -> Result<Vec<UncompiledRawSample>> {
        self.store.list_uncompiled_raw_sample(limit)
    }
    pub fn uncompiled_raw_shelves(&self, limit: usize) -> Result<UncompiledShelfQueue> {
        self.store.uncompiled_raw_shelves(limit)
    }
    pub fn db_file_size_bytes(&self) -> Option<u64> {
        self.store.db_file_size_bytes()
    }
    pub fn wal_file_size_bytes(&self) -> u64 {
        self.store.wal_file_size_bytes()
    }
    pub fn checkpoint(&self) -> Result<()> {
        self.store.checkpoint()
    }
    pub fn integrity_counts(&self) -> Result<(u64, u64, u64, u64, u64)> {
        self.store.integrity_counts()
    }
    pub fn prune_orphans(&self, dry_run: bool) -> Result<(u64, u64, u64)> {
        self.store.prune_orphans(dry_run)
    }
    pub fn vacuum_store(&self) -> Result<VacuumStoreReport> {
        self.store.vacuum_store()
    }
    pub fn schema_version(&self) -> Result<Option<i32>> {
        self.store.schema_version()
    }
    pub fn fts_ready(&self) -> Result<bool> {
        self.store.fts_ready()
    }
    pub fn fts_status(&self) -> Result<Option<FtsState>> {
        self.store.fts_status()
    }
    pub fn reindex_fts(&self, stemmer: Option<&str>) -> Result<FtsState> {
        self.store.reindex_fts(stemmer)
    }
    pub fn chunks_generation(&self) -> Result<u64> {
        self.store.chunks_generation()
    }
    pub fn ensure_fts(&self, stemmer: &str) -> Result<FtsState> {
        self.store.ensure_fts(stemmer)
    }
    pub fn fts_generation_state(&self) -> Result<FtsGenerationState> {
        self.store.fts_generation_state()
    }
    pub fn cleanup_source_duplicates(
        &self,
        dry_run: bool,
        confirm: bool,
        max_candidates: usize,
    ) -> Result<SourceDuplicateCleanupReport> {
        self.store
            .cleanup_source_duplicates(dry_run, confirm, max_candidates)
    }
}

#[derive(Clone, Copy)]
pub struct EmbeddingManifestRepo<'a> {
    pub(in crate::db) store: &'a Store,
}

impl EmbeddingManifestRepo<'_> {
    pub fn get_embedding_manifest(&self) -> Result<Option<EmbeddingManifest>> {
        self.store.get_embedding_manifest()
    }
    pub fn get_embedding_manifest_by_id(&self, id: &str) -> Result<Option<EmbeddingManifest>> {
        self.store.get_embedding_manifest_by_id(id)
    }
    pub fn set_embedding_manifest(&self, manifest: &EmbeddingManifest) -> Result<()> {
        self.store.set_embedding_manifest(manifest)
    }
    pub fn ensure_embedding_manifest(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<EmbeddingManifest> {
        self.store.ensure_embedding_manifest(config)
    }
    pub fn write_embedding_manifest_from_config(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<EmbeddingManifest> {
        self.store.write_embedding_manifest_from_config(config)
    }
    pub fn embedding_manifest_matches_config(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<bool> {
        self.store.embedding_manifest_matches_config(config)
    }
    pub fn require_embedding_manifest_match(&self, config: &impl EmbeddingIdentity) -> Result<()> {
        self.store.require_embedding_manifest_match(config)
    }
    pub fn require_embedding_dims_match(&self, config_dims: usize) -> Result<()> {
        self.store.require_embedding_dims_match(config_dims)
    }
}

#[derive(Clone, Copy)]
pub struct OpsLog<'a> {
    pub(in crate::db) store: &'a Store,
}

impl OpsLog<'_> {
    pub fn append(&self, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
        self.store.append_ops_log(entry)
    }
    pub fn record(&self, entry: &OpsLogEntry) -> Option<OpsLogEntry> {
        match self.append(entry) {
            Ok(written) => Some(written),
            Err(error) => {
                tracing::warn!(%error, op = %entry.op, "failed to append ops_log entry");
                None
            }
        }
    }
    pub fn next_ops_seq(&self) -> Result<i64> {
        self.store.next_ops_seq()
    }
    pub fn append_ops_log(&self, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
        self.store.append_ops_log(entry)
    }
    pub fn ops_log_exists(&self) -> Result<bool> {
        self.store.ops_log_exists()
    }
    pub fn list_ops_log(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.store.list_ops_log(limit)
    }
    pub fn list_recent_ops(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.store.list_recent_ops(limit)
    }
    pub fn memories_filed_away(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.store.memories_filed_away(limit)
    }
    pub fn get_ops_log(&self, id: &str) -> Result<Option<OpsLogEntry>> {
        self.store.get_ops_log(id)
    }
    pub fn get_ops_log_by_seq(&self, seq: i64) -> Result<Option<OpsLogEntry>> {
        self.store.get_ops_log_by_seq(seq)
    }
    pub fn read_ops_log(
        &self,
        id: Option<&str>,
        seq: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<OpsLogEntry>> {
        self.store.read_ops_log(id, seq, limit)
    }
}

#[derive(Clone, Copy)]
pub struct ReembedProgress<'a> {
    pub(in crate::db) store: &'a Store,
}

impl ReembedProgress<'_> {
    pub fn list_reembed_pending_document_ids(
        &self,
        target_fingerprint: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.store
            .list_reembed_pending_document_ids(target_fingerprint, after, limit)
    }
    pub fn count_reembed_pending(&self, target_fingerprint: &str) -> Result<usize> {
        self.store.count_reembed_pending(target_fingerprint)
    }
    pub fn count_reembed_done(&self, target_fingerprint: &str) -> Result<usize> {
        self.store.count_reembed_done(target_fingerprint)
    }
    pub fn mark_reembed_done(
        &self,
        target_fingerprint: &str,
        document_id: &str,
        chunks_reembedded: usize,
    ) -> Result<()> {
        self.store
            .mark_reembed_done(target_fingerprint, document_id, chunks_reembedded)
    }
    pub fn clear_reembed_progress(&self, target_fingerprint: &str) -> Result<usize> {
        self.store.clear_reembed_progress(target_fingerprint)
    }
}

#[derive(Clone, Copy)]
pub struct WikiIndexRepo<'a> {
    pub(in crate::db) store: &'a Store,
}

impl WikiIndexRepo<'_> {
    pub fn upsert_wiki_index_entry(&self, entry: &WikiIndexEntry) -> Result<()> {
        self.store.upsert_wiki_index_entry(entry)
    }
    pub fn list_wiki_index(&self) -> Result<Vec<WikiIndexEntry>> {
        self.store.list_wiki_index()
    }
    pub fn get_wiki_index_by_slug(&self, slug: &str) -> Result<Option<WikiIndexEntry>> {
        self.store.get_wiki_index_by_slug(slug)
    }
    pub fn delete_wiki_index_entry(&self, id: &str) -> Result<bool> {
        self.store.delete_wiki_index_entry(id)
    }
    pub fn rebuild_wiki_index_from_docs(&self) -> Result<usize> {
        self.store.rebuild_wiki_index_from_docs()
    }
    pub fn update_wiki_index_entry_fields(
        &self,
        slug: &str,
        title: Option<String>,
        summary: Option<String>,
        kind: Option<String>,
        category: Option<String>,
        page_id: Option<String>,
    ) -> Result<WikiIndexEntry> {
        self.store
            .update_wiki_index_entry_fields(slug, title, summary, kind, category, page_id)
    }
    pub fn render_wiki_index_markdown(&self) -> Result<String> {
        self.store.render_wiki_index_markdown()
    }
    pub fn query_wiki_index(&self, query: &str, limit: usize) -> Result<Vec<IndexQueryMatch>> {
        self.store.query_wiki_index(query, limit)
    }
}

#[derive(Clone, Copy)]
pub struct Collections<'a> {
    pub(in crate::db) store: &'a Store,
}

impl Collections<'_> {
    pub fn create_collection(
        &self,
        collection: &Collection,
        entries: &[CollectionEntry],
    ) -> Result<CollectionDetail> {
        self.store.create_collection(collection, entries)
    }
    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        self.store.list_collections()
    }
    pub fn get_collection(&self, id: &str) -> Result<Option<CollectionDetail>> {
        self.store.get_collection(id)
    }
    pub fn update_collection(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<Option<&str>>,
        metadata_json: Option<&str>,
        entries: Option<&[CollectionEntry]>,
    ) -> Result<CollectionDetail> {
        self.store
            .update_collection(id, name, description, metadata_json, entries)
    }
}

#[derive(Clone, Copy)]
pub struct GraphRepo<'a> {
    pub(in crate::db) store: &'a Store,
}

impl GraphRepo<'_> {
    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()> {
        self.store.upsert_graph_node(node)
    }
    pub fn set_document_tags(&self, node_id: &str, labels: &[String], replace: bool) -> Result<()> {
        self.store.set_document_tags(node_id, labels, replace)
    }
    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        self.store.insert_graph_edges(edges)
    }
    pub fn delete_edges_from(&self, source_id: &str) -> Result<()> {
        self.store.delete_edges_from(source_id)
    }
    pub fn delete_derived_edges_from(&self, source_id: &str) -> Result<()> {
        self.store.delete_derived_edges_from(source_id)
    }
    pub fn delete_graph_node(&self, node_id: &str) -> Result<bool> {
        self.store.delete_graph_node(node_id)
    }
    pub fn find_node_by_document_id(&self, doc_id: &str) -> Result<Option<GraphNode>> {
        self.store.find_node_by_document_id(doc_id)
    }
    pub fn find_nodes_by_label(&self, label: &str) -> Result<Vec<GraphNode>> {
        self.store.find_nodes_by_label(label)
    }
    pub fn find_node_by_id(&self, node_id: &str) -> Result<Option<GraphNode>> {
        self.store.find_node_by_id(node_id)
    }
    pub fn find_node_by_uri(&self, uri: &str) -> Result<Option<GraphNode>> {
        self.store.find_node_by_uri(uri)
    }
    pub fn resolve_seed(&self, seed: &str) -> Result<Option<GraphNode>> {
        self.store.resolve_seed(seed)
    }
    pub fn resolve_node(
        &self,
        node_id: Option<&str>,
        document_id: Option<&str>,
        label: Option<&str>,
    ) -> Result<Option<GraphNode>> {
        self.store.resolve_node(node_id, document_id, label)
    }
    pub fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView> {
        self.store.get_graph_view(filter)
    }
    pub fn list_graph_nodes(&self) -> Result<Vec<GraphNode>> {
        self.store.list_graph_nodes()
    }
    pub fn list_graph_links(&self) -> Result<Vec<GraphEdge>> {
        self.store.list_graph_links()
    }
    pub fn list_graph_edges(&self) -> Result<Vec<GraphEdge>> {
        self.store.list_graph_edges()
    }
    pub fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        self.store.neighbors(node_id, depth, max_nodes)
    }
    pub fn neighbors_filtered(
        &self,
        node_id: &str,
        depth: u32,
        max_nodes: u32,
        rel_types: Option<&[String]>,
    ) -> Result<GraphView> {
        self.store
            .neighbors_filtered(node_id, depth, max_nodes, rel_types)
    }
    pub fn backlinks(&self, node_id: &str) -> Result<GraphView> {
        self.store.backlinks(node_id)
    }
    pub fn link_nodes(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<GraphEdge> {
        self.store
            .link_nodes(source_id, target_id, rel_type, weight)
    }
    pub fn document_pairs_by_relation(&self, rel_type: &str) -> Result<Vec<(String, String)>> {
        self.store.document_pairs_by_relation(rel_type)
    }
    pub fn graph_stats(&self) -> Result<GraphStats> {
        self.store.graph_stats()
    }
    pub fn prune_orphan_stubs(&self) -> Result<usize> {
        self.store.prune_orphan_stubs()
    }
    pub fn prune_orphan_tags(&self) -> Result<usize> {
        self.store.prune_orphan_tags()
    }
    pub fn export_graph_for_ui(
        &self,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.store.export_graph_for_ui(max_nodes, include_tags)
    }
    pub fn export_project_graph_for_ui(
        &self,
        project: &str,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.store
            .export_project_graph_for_ui(project, max_nodes, include_tags)
    }
    pub fn export_project_neighbors_for_ui(
        &self,
        project: &str,
        seed: &str,
        depth: u32,
        max_nodes: u32,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.store
            .export_project_neighbors_for_ui(project, seed, depth, max_nodes, include_tags)
    }
    pub fn export_pkb_neighbors_for_ui(
        &self,
        seed: &str,
        depth: u32,
        max_nodes: u32,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.store
            .export_pkb_neighbors_for_ui(seed, depth, max_nodes, include_tags)
    }
    pub fn find_seed_node(&self, query: &str) -> Result<Option<GraphNode>> {
        self.store.find_seed_node(query)
    }
    pub fn export_neighbors_for_ui(
        &self,
        seed: &str,
        depth: u32,
        max_nodes: u32,
    ) -> Result<GraphView> {
        self.store.export_neighbors_for_ui(seed, depth, max_nodes)
    }
    pub fn create_tunnel(
        &self,
        source_id: &str,
        target_id: &str,
        weight: f64,
        context: Option<&str>,
    ) -> Result<GraphEdge> {
        self.store
            .create_tunnel(source_id, target_id, weight, context)
    }
    pub fn list_tunnels(&self, node_id: Option<&str>) -> Result<Vec<GraphEdge>> {
        self.store.list_tunnels(node_id)
    }
    pub fn delete_tunnel(&self, tunnel_id: &str) -> Result<bool> {
        self.store.delete_tunnel(tunnel_id)
    }
    pub fn follow_tunnels(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        self.store.follow_tunnels(node_id, depth, max_nodes)
    }
    pub fn find_tunnels(
        &self,
        node_id: Option<&str>,
        other_node_id: Option<&str>,
        wing: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<GraphEdge>> {
        self.store.find_tunnels(node_id, other_node_id, wing, limit)
    }
}

#[derive(Clone, Copy)]
pub struct KgRepo<'a> {
    pub(in crate::db) store: &'a Store,
}

impl KgRepo<'_> {
    pub fn kg_add(&self, fact: KgFactInput<'_>) -> Result<KgFact> {
        self.store.kg_add(fact)
    }
    pub fn kg_query(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
    ) -> Result<Vec<KgFact>> {
        self.store.kg_query(subject, predicate, object, at_time)
    }
    pub fn kg_query_page(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<(Vec<KgFact>, usize)> {
        self.store
            .kg_query_page(subject, predicate, object, at_time, limit)
    }
    pub fn kg_invalidate(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        ended: Option<DateTime<Utc>>,
    ) -> Result<Vec<KgFact>> {
        self.store.kg_invalidate(subject, predicate, object, ended)
    }
    pub fn kg_supersede(&self, change: KgSupersedeInput<'_>) -> Result<KgFact> {
        self.store.kg_supersede(change)
    }
    pub fn kg_timeline(&self, subject: &str) -> Result<Vec<KgFact>> {
        self.store.kg_timeline(subject)
    }
    pub fn kg_stats(&self) -> Result<KgStats> {
        self.store.kg_stats()
    }
    pub fn kg_get(&self, fact_id: &str) -> Result<Option<KgFact>> {
        self.store.kg_get(fact_id)
    }
}

#[derive(Clone, Copy)]
pub struct Recovery<'a> {
    pub(in crate::db) store: &'a Store,
}

impl Recovery<'_> {
    pub fn portable_recovery_preflight(&self) -> Result<PortableBundlePreflight> {
        self.store.portable_recovery_preflight()
    }
    pub fn backup_database(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<BackupReport> {
        self.store.backup_database(path, dry_run, overwrite)
    }
    pub fn recovery_bundle(&self) -> Result<RecoveryBundle> {
        self.store.recovery_bundle()
    }
    pub fn import_recovery_bundle(
        &self,
        bundle: &RecoveryBundle,
        policy: ConflictPolicy,
        dry_run: bool,
        path: &Path,
        format: &str,
    ) -> Result<BundleImportReport> {
        self.store
            .import_recovery_bundle(bundle, policy, dry_run, path, format)
    }
    pub fn export_vault(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<VaultExportReport> {
        self.store.export_vault(path, dry_run, overwrite)
    }
}

#[derive(Clone, Copy)]
pub struct SourceManifestRepo<'a> {
    pub(in crate::db) store: &'a Store,
}

impl SourceManifestRepo<'_> {
    pub fn list_source_roots(&self, project: Option<&str>) -> Result<Vec<SourceRootSummary>> {
        self.store.list_source_roots(project)
    }
    pub fn load_source_manifest_root(
        &self,
        canonical_root: &Path,
    ) -> Result<HashMap<String, SourceManifestEntry>> {
        self.store.load_source_manifest_root(canonical_root)
    }
    pub fn upsert_source_manifest(&self, entry: SourceManifestWrite<'_>) -> Result<()> {
        self.store.upsert_source_manifest(entry)
    }
    pub fn upsert_source_manifests(&self, entries: &[SourceManifestWrite<'_>]) -> Result<()> {
        self.store.upsert_source_manifests(entries)
    }
    pub fn mark_source_manifest_seen(
        &self,
        canonical_root: &Path,
        canonical_paths: &[String],
    ) -> Result<()> {
        self.store
            .mark_source_manifest_seen(canonical_root, canonical_paths)
    }
    pub fn delete_source_manifest(&self, canonical_path: &str) -> Result<bool> {
        self.store.delete_source_manifest(canonical_path)
    }
}

#[derive(Clone, Copy)]
pub struct SyncRepo<'a> {
    pub(in crate::db) store: &'a Store,
}

impl SyncRepo<'_> {
    pub fn has_sync_change(&self, origin_node: &str, origin_seq: i64) -> Result<bool> {
        self.store.has_sync_change(origin_node, origin_seq)
    }
    pub fn previously_applied_sync(&self, mode: &WikiSyncWrite<'_>) -> Result<Option<i64>> {
        self.store.previously_applied_sync(mode)
    }
    pub fn preflight_sync_batch(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<()> {
        self.store.preflight_sync_batch(origin_node, changes)
    }
    pub fn journal_local_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        self.store.journal_local_sync_change(
            entity_kind,
            entity_id,
            operation,
            payload_json,
            content_hash,
        )
    }
    pub fn enqueue_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        self.store.enqueue_sync_change(
            entity_kind,
            entity_id,
            operation,
            payload_json,
            content_hash,
        )
    }
    pub fn pending_sync_outbox(&self, limit: usize) -> Result<Vec<SyncChangeInput>> {
        self.store.pending_sync_outbox(limit)
    }
    pub fn mark_sync_outbox_sent(&self, assignments: &[(i64, i64)]) -> Result<()> {
        self.store.mark_sync_outbox_sent(assignments)
    }
    pub fn register_sync_node(&self, node_id: &str, hostname: &str, role: &str) -> Result<()> {
        self.store.register_sync_node(node_id, hostname, role)
    }
    pub fn accept_sync_changes(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<Vec<i64>> {
        self.store.accept_sync_changes(origin_node, changes)
    }
    pub fn pull_sync_changes(
        &self,
        node_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<SyncChange>> {
        self.store.pull_sync_changes(node_id, after, limit)
    }
    pub fn acknowledge_sync_cursor(&self, node_id: &str, cursor: i64) -> Result<()> {
        self.store.acknowledge_sync_cursor(node_id, cursor)
    }
    pub fn sync_pull_cursor(&self, node_id: &str) -> Result<i64> {
        self.store.sync_pull_cursor(node_id)
    }
    pub fn sync_status(
        &self,
        role: &str,
        node_id: &str,
        primary_url: Option<String>,
    ) -> Result<SyncStatus> {
        self.store.sync_status(role, node_id, primary_url)
    }
}
