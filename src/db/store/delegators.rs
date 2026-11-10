//! Temporary Store methods that forward to the repositories while call
//! sites move to `store.<repo>().<method>()`. Deleted at the end of the move.

use super::*;
#[allow(unused_imports)]
use crate::db::{
    catalog::*, collections::*, graph::*, kg::*, recovery::*, source_dedupe::*, source_manifest::*,
    sync::*, vault::*,
};
#[allow(unused_imports)]
use crate::models::*;
#[allow(unused_imports)]
use chrono::DateTime;
#[allow(unused_imports)]
use std::collections::HashMap;

#[allow(dead_code)]
impl Store {
    pub(crate) fn delete_source_state_ids_with(
        &self,
        source: &str,
        after_document: impl FnMut(usize) -> Result<()>,
    ) -> Result<(Vec<String>, bool)> {
        self.documents()
            .delete_source_state_ids_with(source, after_document)
    }

    pub(crate) fn delete_source_state_with(
        &self,
        source: &str,
        after_document: impl FnMut(usize) -> Result<()>,
    ) -> Result<(u64, bool)> {
        self.documents()
            .delete_source_state_with(source, after_document)
    }

    pub(crate) fn delete_source_state(&self, source: &str) -> Result<(u64, bool)> {
        self.documents().delete_source_state(source)
    }

    pub fn delete_by_source_ids(&self, source: &str) -> Result<Vec<String>> {
        self.documents().delete_by_source_ids(source)
    }

    pub fn delete_by_source(&self, source: &str) -> Result<u64> {
        self.documents().delete_by_source(source)
    }

    pub(crate) fn merge_exact_duplicates_atomic_with(
        &self,
        keep_id: &str,
        source_ids: &[String],
        declared_hash: Option<&str>,
        hard_delete: bool,
        allow_raw_delete: bool,
        after_mutation: impl FnMut(usize) -> Result<()>,
    ) -> Result<ExactDuplicateMergeResult> {
        self.documents().merge_exact_duplicates_atomic_with(
            keep_id,
            source_ids,
            declared_hash,
            hard_delete,
            allow_raw_delete,
            after_mutation,
        )
    }

    pub(crate) fn merge_exact_duplicates_atomic(
        &self,
        keep_id: &str,
        source_ids: &[String],
        declared_hash: Option<&str>,
        hard_delete: bool,
        allow_raw_delete: bool,
    ) -> Result<ExactDuplicateMergeResult> {
        self.documents().merge_exact_duplicates_atomic(
            keep_id,
            source_ids,
            declared_hash,
            hard_delete,
            allow_raw_delete,
        )
    }

    pub(crate) fn apply_document_dispositions_atomic(
        &self,
        delete_ids: &[String],
        tombstone_ids: &[String],
    ) -> Result<(u64, u64, u64)> {
        self.documents()
            .apply_document_dispositions_atomic(delete_ids, tombstone_ids)
    }

    pub fn delete_document(&self, id: &str) -> Result<bool> {
        self.documents().delete_document(id)
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn write_wiki_document_atomic(
        &self,
        write: WikiDocumentWrite<'_>,
    ) -> Result<(Option<AtomicDocumentWriteResult>, i64)> {
        self.documents().write_wiki_document_atomic(write)
    }

    pub(crate) fn write_document_atomic_with_manifest(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
        manifest: Option<crate::db::SourceManifestWrite<'_>>,
        sync_identity: Option<&crate::db::sync::SyncIdentity>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.documents().write_document_atomic_with_manifest(
            doc,
            if_match_revision,
            derived,
            manifest,
            sync_identity,
        )
    }

    pub(crate) fn write_source_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        chunks: &[Chunk],
        manifest: crate::db::SourceManifestWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.documents()
            .write_source_document_atomic(doc, if_match_revision, chunks, manifest)
    }

    pub(crate) fn write_unchunked_document_with_audit(
        &self,
        doc: &Document,
        audit: &OpsLogEntry,
    ) -> Result<AtomicDocumentWriteResult> {
        self.documents()
            .write_unchunked_document_with_audit(doc, audit)
    }

    pub(crate) fn write_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.documents()
            .write_document_atomic(doc, if_match_revision, derived)
    }

    pub fn upsert_document_cas(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
    ) -> Result<i64> {
        self.documents().upsert_document_cas(doc, if_match_revision)
    }

    pub fn upsert_document(&self, doc: &Document) -> Result<()> {
        self.documents().upsert_document(doc)
    }

    pub fn update_document_placement(
        &self,
        id: &str,
        update: &PlacementUpdate,
    ) -> Result<Option<Document>> {
        self.documents().update_document_placement(id, update)
    }

    pub(crate) fn prepare_document_meta_update(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.documents().prepare_document_meta_update(id, update)
    }

    pub(crate) fn update_document_meta_with_identity(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: Option<&crate::db::sync::SyncIdentity>,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.documents()
            .update_document_meta_with_identity(id, update, identity)
    }

    #[cfg(test)]
    pub(crate) fn update_document_meta_for_sync_test(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: &crate::db::sync::SyncIdentity,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.documents()
            .update_document_meta_for_sync_test(id, update, identity)
    }

    pub fn update_document_meta(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.documents().update_document_meta(id, update)
    }

    pub fn get_taxonomy(&self) -> Result<Taxonomy> {
        self.documents().get_taxonomy()
    }

    pub fn list_rooms(&self, wing: Option<&str>) -> Result<Vec<RoomCount>> {
        self.documents().list_rooms(wing)
    }

    pub fn list_projects(&self) -> Result<Vec<crate::models::ProjectSummary>> {
        self.documents().list_projects()
    }

    pub fn list_wings(&self) -> Result<Vec<WingCount>> {
        self.documents().list_wings()
    }

    pub fn list_documents_filtered(&self, filter: &DocumentFilter) -> Result<Vec<Document>> {
        self.documents().list_documents_filtered(filter)
    }

    pub fn list_documents_by_wing(&self, wing: &str) -> Result<Vec<Document>> {
        self.documents().list_documents_by_wing(wing)
    }

    pub fn wiki_backlinks_for_document_in_wing(
        &self,
        document_id: &str,
        wing: &str,
    ) -> Result<Vec<(String, String)>> {
        self.documents()
            .wiki_backlinks_for_document_in_wing(document_id, wing)
    }

    pub fn wiki_backlink_view(
        &self,
        document_id: &str,
    ) -> Result<Option<(String, crate::models::GraphView)>> {
        self.documents().wiki_backlink_view(document_id)
    }

    pub fn wiki_backlinks_for_document(&self, document_id: &str) -> Result<Vec<(String, String)>> {
        self.documents().wiki_backlinks_for_document(document_id)
    }

    pub fn list_wiki_page_metas_filtered(
        &self,
        filter: &WikiPageMetaFilter,
    ) -> Result<(Vec<crate::models::WikiPageListItem>, usize)> {
        self.documents().list_wiki_page_metas_filtered(filter)
    }

    pub fn list_wiki_page_metas(&self) -> Result<Vec<crate::models::WikiPageListItem>> {
        self.documents().list_wiki_page_metas()
    }

    pub(crate) fn document_contents(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        self.documents().document_contents(ids)
    }

    pub(crate) fn list_documents_by_layer_without_body(
        &self,
        layer: &str,
    ) -> Result<Vec<Document>> {
        self.documents().list_documents_by_layer_without_body(layer)
    }

    pub fn list_documents_by_layer(&self, layer: &str) -> Result<Vec<Document>> {
        self.documents().list_documents_by_layer(layer)
    }

    pub(crate) fn list_documents_missing_chunks(
        &self,
        limit: usize,
    ) -> Result<(usize, Vec<Document>)> {
        self.documents().list_documents_missing_chunks(limit)
    }

    pub(crate) fn list_active_unscoped_documents(
        &self,
        limit: usize,
    ) -> Result<(usize, Vec<Document>)> {
        self.documents().list_active_unscoped_documents(limit)
    }

    pub(crate) fn exact_duplicate_source_ids(
        &self,
        hash: &str,
        keep_id: &str,
        limit: usize,
    ) -> Result<(usize, Vec<String>)> {
        self.documents()
            .exact_duplicate_source_ids(hash, keep_id, limit)
    }

    pub(crate) fn count_chunks_for_document_limit(&self, limit: usize) -> Result<usize> {
        self.documents().count_chunks_for_document_limit(limit)
    }

    pub(crate) fn list_document_ids_after(
        &self,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.documents().list_document_ids_after(after_id, limit)
    }

    pub(crate) fn list_graph_rebuild_candidates_after(
        &self,
        dirty_only: bool,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Document>> {
        self.documents()
            .list_graph_rebuild_candidates_after(dirty_only, after_id, limit)
    }

    pub(crate) fn count_graph_rebuild_candidates(&self, dirty_only: bool) -> Result<usize> {
        self.documents().count_graph_rebuild_candidates(dirty_only)
    }

    pub(crate) fn count_documents_by_layer(&self, layer: &str) -> Result<usize> {
        self.documents().count_documents_by_layer(layer)
    }

    pub(crate) fn count_documents(&self) -> Result<usize> {
        self.documents().count_documents()
    }

    pub(crate) fn list_document_refs(&self) -> Result<Vec<DocumentRef>> {
        self.documents().list_document_refs()
    }

    pub(crate) fn document_id_and_uri(&self, id: &str) -> Result<Option<(String, String)>> {
        self.documents().document_id_and_uri(id)
    }

    pub(crate) fn document_revision(&self, id: &str) -> Result<Option<i64>> {
        self.documents().document_revision(id)
    }

    pub fn list_documents(&self) -> Result<Vec<Document>> {
        self.documents().list_documents()
    }

    pub fn check_duplicate(
        &self,
        content: Option<&str>,
        content_hash_in: Option<&str>,
        uri: Option<&str>,
    ) -> Result<DuplicateCheckResult> {
        self.documents()
            .check_duplicate(content, content_hash_in, uri)
    }

    pub fn find_all_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        self.documents().find_all_by_content_hash(hash)
    }

    pub fn list_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        self.documents().list_by_content_hash(hash)
    }

    pub fn find_by_content_hash(&self, hash: &str) -> Result<Option<Document>> {
        self.documents().find_by_content_hash(hash)
    }

    pub fn content_hashes(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Option<String>>> {
        self.documents().content_hashes(ids)
    }

    pub fn find_by_id_or_uri(&self, id: &str, uri: &str) -> Result<Option<Document>> {
        self.documents().find_by_id_or_uri(id, uri)
    }

    pub fn find_by_uri(&self, uri: &str) -> Result<Option<Document>> {
        self.documents().find_by_uri(uri)
    }

    pub fn get_document(&self, id: &str) -> Result<Option<Document>> {
        self.documents().get_document(id)
    }

    pub fn get_document_revision(
        &self,
        document_id: &str,
        revision: i64,
    ) -> Result<Option<Document>> {
        self.documents()
            .get_document_revision(document_id, revision)
    }

    pub fn list_document_revision_summaries(
        &self,
        document_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<DocumentRevisionPage> {
        self.documents()
            .list_document_revision_summaries(document_id, limit, offset)
    }

    pub fn list_document_revisions(&self, document_id: &str) -> Result<Vec<Document>> {
        self.documents().list_document_revisions(document_id)
    }

    pub fn list_pinned_documents(&self, limit: usize) -> Result<Vec<Document>> {
        self.documents().list_pinned_documents(limit)
    }

    pub(crate) fn list_diary_entries_with(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        self.documents().list_diary_entries_with(agent_name, limit)
    }

    pub(crate) fn list_diary_activity(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        self.documents().list_diary_activity(agent_name, limit)
    }

    pub fn list_diary_entries(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        self.documents().list_diary_entries(agent_name, limit)
    }

    pub fn diary_wing_for_agent(agent_name: &str) -> String {
        Documents::diary_wing_for_agent(agent_name)
    }

    pub fn get_schema_document(&self) -> Result<Option<Document>> {
        self.documents().get_schema_document()
    }

    pub fn project_home(&self, project_id: &str) -> Result<ProjectHome> {
        self.documents().project_home(project_id)
    }

    pub fn list_document_catalog(
        &self,
        filter: &DocumentCatalogFilter,
    ) -> Result<DocumentCatalogPage> {
        self.documents().list_document_catalog(filter)
    }

    pub fn delete_chunks_for_document(&self, document_id: &str) -> Result<()> {
        self.chunks().delete_chunks_for_document(document_id)
    }

    pub(crate) fn count_chunks_for_document(&self, document_id: &str) -> Result<u64> {
        self.chunks().count_chunks_for_document(document_id)
    }

    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        self.chunks().insert_chunks(chunks)
    }

    pub(crate) fn update_chunk_embeddings_atomic_with(
        &self,
        document_id: &str,
        if_match_revision: i64,
        expected_chunks: &[Chunk],
        embeddings: &[Vec<f32>],
        manifest: Option<&EmbeddingManifest>,
        after_chunk_update: impl FnMut(usize) -> Result<()>,
    ) -> Result<usize> {
        self.chunks().update_chunk_embeddings_atomic_with(
            document_id,
            if_match_revision,
            expected_chunks,
            embeddings,
            manifest,
            after_chunk_update,
        )
    }

    pub(crate) fn update_chunk_embeddings_atomic(
        &self,
        document_id: &str,
        if_match_revision: i64,
        expected_chunks: &[Chunk],
        embeddings: &[Vec<f32>],
        manifest: Option<&EmbeddingManifest>,
    ) -> Result<usize> {
        self.chunks().update_chunk_embeddings_atomic(
            document_id,
            if_match_revision,
            expected_chunks,
            embeddings,
            manifest,
        )
    }

    pub fn replace_chunks_for_document(&self, document_id: &str, chunks: &[Chunk]) -> Result<()> {
        self.chunks()
            .replace_chunks_for_document(document_id, chunks)
    }

    pub fn all_chunks_with_embeddings(&self) -> Result<Vec<Chunk>> {
        self.chunks().all_chunks_with_embeddings()
    }

    pub(crate) fn first_chunk_embeddings(
        &self,
        document_ids: &[&str],
    ) -> Result<std::collections::BTreeMap<String, Vec<f32>>> {
        self.chunks().first_chunk_embeddings(document_ids)
    }

    pub(crate) fn representative_document_embeddings(
        &self,
        limit: usize,
        active_only: bool,
        require_wing: bool,
        exclude_system_layers: bool,
    ) -> Result<Vec<DocumentEmbeddingSample>> {
        self.chunks().representative_document_embeddings(
            limit,
            active_only,
            require_wing,
            exclude_system_layers,
        )
    }

    pub(crate) fn chunk_metadata_by_ids(
        &self,
        chunk_ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        self.chunks().chunk_metadata_by_ids(chunk_ids)
    }

    pub(crate) fn chunk_vectors_by_content(
        &self,
        document_ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Vec<f32>>> {
        self.chunks().chunk_vectors_by_content(document_ids)
    }

    pub(crate) fn list_chunk_bodies_in_range(
        &self,
        doc_id: &str,
        low: i32,
        high: i32,
    ) -> Result<Vec<ChunkBody>> {
        self.chunks().list_chunk_bodies_in_range(doc_id, low, high)
    }

    pub(crate) fn list_chunk_bodies_for_document(&self, doc_id: &str) -> Result<Vec<ChunkBody>> {
        self.chunks().list_chunk_bodies_for_document(doc_id)
    }

    pub fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>> {
        self.chunks().list_chunks_for_document(doc_id)
    }

    pub fn prune_orphan_tags(&self) -> Result<usize> {
        self.graph().prune_orphan_tags()
    }

    pub fn prune_orphan_stubs(&self) -> Result<usize> {
        self.graph().prune_orphan_stubs()
    }

    pub fn graph_stats(&self) -> Result<GraphStats> {
        self.graph().graph_stats()
    }

    pub fn document_pairs_by_relation(&self, rel_type: &str) -> Result<Vec<(String, String)>> {
        self.graph().document_pairs_by_relation(rel_type)
    }

    pub fn link_nodes(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<GraphEdge> {
        self.graph()
            .link_nodes(source_id, target_id, rel_type, weight)
    }

    pub fn backlinks(&self, node_id: &str) -> Result<GraphView> {
        self.graph().backlinks(node_id)
    }

    pub fn neighbors_filtered(
        &self,
        node_id: &str,
        depth: u32,
        max_nodes: u32,
        rel_types: Option<&[String]>,
    ) -> Result<GraphView> {
        self.graph()
            .neighbors_filtered(node_id, depth, max_nodes, rel_types)
    }

    pub fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        self.graph().neighbors(node_id, depth, max_nodes)
    }

    pub fn list_graph_edges(&self) -> Result<Vec<GraphEdge>> {
        self.graph().list_graph_edges()
    }

    pub fn list_graph_links(&self) -> Result<Vec<GraphEdge>> {
        self.graph().list_graph_links()
    }

    pub fn list_graph_nodes(&self) -> Result<Vec<GraphNode>> {
        self.graph().list_graph_nodes()
    }

    pub fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView> {
        self.graph().get_graph_view(filter)
    }

    pub fn resolve_node(
        &self,
        node_id: Option<&str>,
        document_id: Option<&str>,
        label: Option<&str>,
    ) -> Result<Option<GraphNode>> {
        self.graph().resolve_node(node_id, document_id, label)
    }

    pub fn resolve_seed(&self, seed: &str) -> Result<Option<GraphNode>> {
        self.graph().resolve_seed(seed)
    }

    pub fn find_node_by_uri(&self, uri: &str) -> Result<Option<GraphNode>> {
        self.graph().find_node_by_uri(uri)
    }

    pub fn find_node_by_id(&self, node_id: &str) -> Result<Option<GraphNode>> {
        self.graph().find_node_by_id(node_id)
    }

    pub fn find_nodes_by_label(&self, label: &str) -> Result<Vec<GraphNode>> {
        self.graph().find_nodes_by_label(label)
    }

    pub fn find_node_by_document_id(&self, doc_id: &str) -> Result<Option<GraphNode>> {
        self.graph().find_node_by_document_id(doc_id)
    }

    pub fn delete_graph_node(&self, node_id: &str) -> Result<bool> {
        self.graph().delete_graph_node(node_id)
    }

    pub fn delete_derived_edges_from(&self, source_id: &str) -> Result<()> {
        self.graph().delete_derived_edges_from(source_id)
    }

    pub fn delete_edges_from(&self, source_id: &str) -> Result<()> {
        self.graph().delete_edges_from(source_id)
    }

    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        self.graph().insert_graph_edges(edges)
    }

    pub fn set_document_tags(&self, node_id: &str, labels: &[String], replace: bool) -> Result<()> {
        self.graph().set_document_tags(node_id, labels, replace)
    }

    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()> {
        self.graph().upsert_graph_node(node)
    }

    pub(crate) fn find_tunnel_between(&self, a: &str, b: &str) -> Result<Option<GraphEdge>> {
        self.graph().find_tunnel_between(a, b)
    }

    pub(crate) fn nodes_in_wing(
        &self,
        node_ids: &[&str],
        wing: &str,
    ) -> Result<std::collections::HashSet<String>> {
        self.graph().nodes_in_wing(node_ids, wing)
    }

    pub fn find_tunnels(
        &self,
        node_id: Option<&str>,
        other_node_id: Option<&str>,
        wing: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<GraphEdge>> {
        self.graph()
            .find_tunnels(node_id, other_node_id, wing, limit)
    }

    pub fn follow_tunnels(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        self.graph().follow_tunnels(node_id, depth, max_nodes)
    }

    pub fn delete_tunnel(&self, tunnel_id: &str) -> Result<bool> {
        self.graph().delete_tunnel(tunnel_id)
    }

    pub fn list_tunnels(&self, node_id: Option<&str>) -> Result<Vec<GraphEdge>> {
        self.graph().list_tunnels(node_id)
    }

    pub fn create_tunnel(
        &self,
        source_id: &str,
        target_id: &str,
        weight: f64,
        context: Option<&str>,
    ) -> Result<GraphEdge> {
        self.graph()
            .create_tunnel(source_id, target_id, weight, context)
    }

    pub fn export_neighbors_for_ui(
        &self,
        seed: &str,
        depth: u32,
        max_nodes: u32,
    ) -> Result<GraphView> {
        self.graph().export_neighbors_for_ui(seed, depth, max_nodes)
    }

    pub fn find_seed_node(&self, query: &str) -> Result<Option<GraphNode>> {
        self.graph().find_seed_node(query)
    }

    pub fn export_pkb_neighbors_for_ui(
        &self,
        seed: &str,
        depth: u32,
        max_nodes: u32,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.graph()
            .export_pkb_neighbors_for_ui(seed, depth, max_nodes, include_tags)
    }

    pub fn export_project_neighbors_for_ui(
        &self,
        project: &str,
        seed: &str,
        depth: u32,
        max_nodes: u32,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.graph()
            .export_project_neighbors_for_ui(project, seed, depth, max_nodes, include_tags)
    }

    pub fn export_project_graph_for_ui(
        &self,
        project: &str,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.graph()
            .export_project_graph_for_ui(project, max_nodes, include_tags)
    }

    pub fn export_graph_for_ui(
        &self,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.graph().export_graph_for_ui(max_nodes, include_tags)
    }

    pub fn kg_get(&self, fact_id: &str) -> Result<Option<KgFact>> {
        self.kg().kg_get(fact_id)
    }

    pub fn kg_stats(&self) -> Result<KgStats> {
        self.kg().kg_stats()
    }

    pub fn kg_timeline(&self, subject: &str) -> Result<Vec<KgFact>> {
        self.kg().kg_timeline(subject)
    }

    pub fn kg_supersede(&self, change: KgSupersedeInput<'_>) -> Result<KgFact> {
        self.kg().kg_supersede(change)
    }

    pub fn kg_invalidate(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        ended: Option<DateTime<Utc>>,
    ) -> Result<Vec<KgFact>> {
        self.kg().kg_invalidate(subject, predicate, object, ended)
    }

    pub fn kg_query_page(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<(Vec<KgFact>, usize)> {
        self.kg()
            .kg_query_page(subject, predicate, object, at_time, limit)
    }

    pub fn kg_query(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
    ) -> Result<Vec<KgFact>> {
        self.kg().kg_query(subject, predicate, object, at_time)
    }

    pub(crate) fn kg_query_sql(
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
    ) -> Result<(String, Vec<String>)> {
        KgRepo::kg_query_sql(subject, predicate, object, at_time)
    }

    pub fn kg_add(&self, fact: KgFactInput<'_>) -> Result<KgFact> {
        self.kg().kg_add(fact)
    }

    pub fn read_ops_log(
        &self,
        id: Option<&str>,
        seq: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<OpsLogEntry>> {
        self.ops_log().read_ops_log(id, seq, limit)
    }

    pub fn get_ops_log_by_seq(&self, seq: i64) -> Result<Option<OpsLogEntry>> {
        self.ops_log().get_ops_log_by_seq(seq)
    }

    pub fn get_ops_log(&self, id: &str) -> Result<Option<OpsLogEntry>> {
        self.ops_log().get_ops_log(id)
    }

    pub fn memories_filed_away(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.ops_log().memories_filed_away(limit)
    }

    pub fn list_recent_ops(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.ops_log().list_recent_ops(limit)
    }

    pub fn list_ops_log(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.ops_log().list_ops_log(limit)
    }

    pub fn ops_log_exists(&self) -> Result<bool> {
        self.ops_log().ops_log_exists()
    }

    pub fn append_ops_log(&self, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
        self.ops_log().append_ops_log(entry)
    }

    pub fn next_ops_seq(&self) -> Result<i64> {
        self.ops_log().next_ops_seq()
    }

    pub fn require_embedding_dims_match(&self, config_dims: usize) -> Result<()> {
        self.embedding_manifest()
            .require_embedding_dims_match(config_dims)
    }

    pub fn require_embedding_manifest_match(&self, config: &impl EmbeddingIdentity) -> Result<()> {
        self.embedding_manifest()
            .require_embedding_manifest_match(config)
    }

    pub fn embedding_manifest_matches_config(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<bool> {
        self.embedding_manifest()
            .embedding_manifest_matches_config(config)
    }

    pub fn write_embedding_manifest_from_config(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<EmbeddingManifest> {
        self.embedding_manifest()
            .write_embedding_manifest_from_config(config)
    }

    pub fn ensure_embedding_manifest(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<EmbeddingManifest> {
        self.embedding_manifest().ensure_embedding_manifest(config)
    }

    pub fn set_embedding_manifest(&self, manifest: &EmbeddingManifest) -> Result<()> {
        self.embedding_manifest().set_embedding_manifest(manifest)
    }

    pub fn get_embedding_manifest_by_id(&self, id: &str) -> Result<Option<EmbeddingManifest>> {
        self.embedding_manifest().get_embedding_manifest_by_id(id)
    }

    pub fn get_embedding_manifest(&self) -> Result<Option<EmbeddingManifest>> {
        self.embedding_manifest().get_embedding_manifest()
    }

    pub fn query_wiki_index(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::models::IndexQueryMatch>> {
        self.wiki_index().query_wiki_index(query, limit)
    }

    pub fn render_wiki_index_markdown(&self) -> Result<String> {
        self.wiki_index().render_wiki_index_markdown()
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
        self.wiki_index()
            .update_wiki_index_entry_fields(slug, title, summary, kind, category, page_id)
    }

    pub fn rebuild_wiki_index_from_docs(&self) -> Result<usize> {
        self.wiki_index().rebuild_wiki_index_from_docs()
    }

    pub fn delete_wiki_index_entry(&self, id: &str) -> Result<bool> {
        self.wiki_index().delete_wiki_index_entry(id)
    }

    pub fn get_wiki_index_by_slug(&self, slug: &str) -> Result<Option<WikiIndexEntry>> {
        self.wiki_index().get_wiki_index_by_slug(slug)
    }

    pub fn list_wiki_index(&self) -> Result<Vec<WikiIndexEntry>> {
        self.wiki_index().list_wiki_index()
    }

    pub fn upsert_wiki_index_entry(&self, entry: &WikiIndexEntry) -> Result<()> {
        self.wiki_index().upsert_wiki_index_entry(entry)
    }

    pub fn sync_status(
        &self,
        role: &str,
        node_id: &str,
        primary_url: Option<String>,
    ) -> Result<SyncStatus> {
        self.sync().sync_status(role, node_id, primary_url)
    }

    pub fn sync_pull_cursor(&self, node_id: &str) -> Result<i64> {
        self.sync().sync_pull_cursor(node_id)
    }

    pub fn acknowledge_sync_cursor(&self, node_id: &str, cursor: i64) -> Result<()> {
        self.sync().acknowledge_sync_cursor(node_id, cursor)
    }

    pub fn pull_sync_changes(
        &self,
        node_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<SyncChange>> {
        self.sync().pull_sync_changes(node_id, after, limit)
    }

    pub fn accept_sync_changes(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<Vec<i64>> {
        self.sync().accept_sync_changes(origin_node, changes)
    }

    pub fn register_sync_node(&self, node_id: &str, hostname: &str, role: &str) -> Result<()> {
        self.sync().register_sync_node(node_id, hostname, role)
    }

    pub fn mark_sync_outbox_sent(&self, assignments: &[(i64, i64)]) -> Result<()> {
        self.sync().mark_sync_outbox_sent(assignments)
    }

    pub fn pending_sync_outbox(&self, limit: usize) -> Result<Vec<SyncChangeInput>> {
        self.sync().pending_sync_outbox(limit)
    }

    pub fn enqueue_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        self.sync().enqueue_sync_change(
            entity_kind,
            entity_id,
            operation,
            payload_json,
            content_hash,
        )
    }

    pub fn journal_local_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        self.sync().journal_local_sync_change(
            entity_kind,
            entity_id,
            operation,
            payload_json,
            content_hash,
        )
    }

    pub(crate) fn preflight_sync_batch(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<()> {
        self.sync().preflight_sync_batch(origin_node, changes)
    }

    pub(crate) fn previously_applied_sync(&self, mode: &WikiSyncWrite<'_>) -> Result<Option<i64>> {
        self.sync().previously_applied_sync(mode)
    }

    pub fn has_sync_change(&self, origin_node: &str, origin_seq: i64) -> Result<bool> {
        self.sync().has_sync_change(origin_node, origin_seq)
    }

    pub(crate) fn record_fts_finalization_failure_inner(
        &self,
        operation: &str,
        durable_mutation_committed: bool,
        error: &str,
    ) -> FtsFinalizationFailure {
        self.corpus_lane().record_fts_finalization_failure_inner(
            operation,
            durable_mutation_committed,
            error,
        )
    }

    pub(crate) fn record_fts_finalization_failure(
        &self,
        operation: &str,
        durable_mutation_committed: bool,
        error: &str,
    ) -> FtsFinalizationFailure {
        self.corpus_lane().record_fts_finalization_failure(
            operation,
            durable_mutation_committed,
            error,
        )
    }

    pub(crate) fn mark_fts_dirty_for_retry(&self) -> Result<()> {
        self.corpus_lane().mark_fts_dirty_for_retry()
    }

    pub(crate) fn try_embedding_write_guard(
        &self,
        config: &impl EmbeddingIdentity,
        operation: &str,
    ) -> Result<EmbeddingWriteGuard> {
        self.corpus_lane()
            .try_embedding_write_guard(config, operation)
    }

    pub(crate) fn try_corpus_mutation_scope(&self, operation: &str) -> Result<Self> {
        self.corpus_lane().try_corpus_mutation_scope(operation)
    }

    pub(crate) fn with_corpus_mutation_guard(&self, guard: CorpusMutationGuard) -> Result<Self> {
        self.corpus_lane().with_corpus_mutation_guard(guard)
    }

    pub(crate) fn try_corpus_idle_guard(&self) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        self.corpus_lane().try_corpus_idle_guard()
    }

    pub(crate) fn try_corpus_mutation_guard(&self, operation: &str) -> Result<CorpusMutationGuard> {
        self.corpus_lane().try_corpus_mutation_guard(operation)
    }

    pub(crate) fn corpus_mutation_lane(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.corpus_lane().corpus_mutation_lane()
    }

    pub fn update_collection(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<Option<&str>>,
        metadata_json: Option<&str>,
        entries: Option<&[CollectionEntry]>,
    ) -> Result<CollectionDetail> {
        self.collections()
            .update_collection(id, name, description, metadata_json, entries)
    }

    pub fn get_collection(&self, id: &str) -> Result<Option<CollectionDetail>> {
        self.collections().get_collection(id)
    }

    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        self.collections().list_collections()
    }

    pub fn create_collection(
        &self,
        collection: &Collection,
        entries: &[CollectionEntry],
    ) -> Result<CollectionDetail> {
        self.collections().create_collection(collection, entries)
    }

    pub(crate) fn clear_reembed_progress(&self, target_fingerprint: &str) -> Result<usize> {
        self.reembed_progress()
            .clear_reembed_progress(target_fingerprint)
    }

    pub(crate) fn mark_reembed_done(
        &self,
        target_fingerprint: &str,
        document_id: &str,
        chunks_reembedded: usize,
    ) -> Result<()> {
        self.reembed_progress().mark_reembed_done(
            target_fingerprint,
            document_id,
            chunks_reembedded,
        )
    }

    pub(crate) fn count_reembed_done(&self, target_fingerprint: &str) -> Result<usize> {
        self.reembed_progress()
            .count_reembed_done(target_fingerprint)
    }

    pub(crate) fn count_reembed_pending(&self, target_fingerprint: &str) -> Result<usize> {
        self.reembed_progress()
            .count_reembed_pending(target_fingerprint)
    }

    pub(crate) fn list_reembed_pending_document_ids(
        &self,
        target_fingerprint: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.reembed_progress()
            .list_reembed_pending_document_ids(target_fingerprint, after, limit)
    }

    pub fn fts_generation_state(&self) -> Result<crate::db::fts::FtsGenerationState> {
        self.maintenance().fts_generation_state()
    }

    pub fn ensure_fts(&self, stemmer: &str) -> Result<crate::db::fts::FtsState> {
        self.maintenance().ensure_fts(stemmer)
    }

    pub fn chunks_generation(&self) -> Result<u64> {
        self.maintenance().chunks_generation()
    }

    pub fn reindex_fts(&self, stemmer: Option<&str>) -> Result<crate::db::fts::FtsState> {
        self.maintenance().reindex_fts(stemmer)
    }

    pub fn fts_status(&self) -> Result<Option<crate::db::fts::FtsState>> {
        self.maintenance().fts_status()
    }

    pub fn fts_ready(&self) -> Result<bool> {
        self.maintenance().fts_ready()
    }

    pub fn schema_version(&self) -> Result<Option<i32>> {
        self.maintenance().schema_version()
    }

    pub fn vacuum_store(&self) -> Result<VacuumStoreReport> {
        self.maintenance().vacuum_store()
    }

    #[cfg(test)]
    pub fn prune_orphans(&self, dry_run: bool) -> Result<(u64, u64, u64)> {
        self.maintenance().prune_orphans(dry_run)
    }

    pub fn integrity_counts(&self) -> Result<(u64, u64, u64, u64, u64)> {
        self.maintenance().integrity_counts()
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.maintenance().checkpoint()
    }

    pub fn wal_file_size_bytes(&self) -> u64 {
        self.maintenance().wal_file_size_bytes()
    }

    pub fn db_file_size_bytes(&self) -> Option<u64> {
        self.maintenance().db_file_size_bytes()
    }

    pub(crate) fn uncompiled_raw_shelves(&self, limit: usize) -> Result<UncompiledShelfQueue> {
        self.maintenance().uncompiled_raw_shelves(limit)
    }

    pub fn list_uncompiled_raw_sample(&self, limit: usize) -> Result<Vec<UncompiledRawSample>> {
        self.maintenance().list_uncompiled_raw_sample(limit)
    }

    pub(crate) fn layer_health_counts(&self) -> Result<LayerHealthCounts> {
        self.maintenance().layer_health_counts()
    }

    pub fn stats(&self) -> Result<(u64, u64, u64, u64)> {
        self.maintenance().stats()
    }

    pub fn cleanup_source_duplicates(
        &self,
        dry_run: bool,
        confirm: bool,
        max_candidates: usize,
    ) -> Result<SourceDuplicateCleanupReport> {
        self.maintenance()
            .cleanup_source_duplicates(dry_run, confirm, max_candidates)
    }

    pub fn delete_source_manifest(&self, canonical_path: &str) -> Result<bool> {
        self.source_manifest()
            .delete_source_manifest(canonical_path)
    }

    pub fn mark_source_manifest_seen(
        &self,
        canonical_root: &Path,
        canonical_paths: &[String],
    ) -> Result<()> {
        self.source_manifest()
            .mark_source_manifest_seen(canonical_root, canonical_paths)
    }

    pub fn upsert_source_manifests(&self, entries: &[SourceManifestWrite<'_>]) -> Result<()> {
        self.source_manifest().upsert_source_manifests(entries)
    }

    pub fn upsert_source_manifest(&self, entry: SourceManifestWrite<'_>) -> Result<()> {
        self.source_manifest().upsert_source_manifest(entry)
    }

    pub fn load_source_manifest_root(
        &self,
        canonical_root: &Path,
    ) -> Result<HashMap<String, SourceManifestEntry>> {
        self.source_manifest()
            .load_source_manifest_root(canonical_root)
    }

    pub fn list_source_roots(&self, project: Option<&str>) -> Result<Vec<SourceRootSummary>> {
        self.source_manifest().list_source_roots(project)
    }

    pub fn import_recovery_bundle(
        &self,
        bundle: &RecoveryBundle,
        policy: ConflictPolicy,
        dry_run: bool,
        path: &Path,
        format: &str,
    ) -> Result<BundleImportReport> {
        self.recovery()
            .import_recovery_bundle(bundle, policy, dry_run, path, format)
    }

    pub fn recovery_bundle(&self) -> Result<RecoveryBundle> {
        self.recovery().recovery_bundle()
    }

    pub fn backup_database(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<BackupReport> {
        self.recovery().backup_database(path, dry_run, overwrite)
    }

    pub fn portable_recovery_preflight(&self) -> Result<PortableBundlePreflight> {
        self.recovery().portable_recovery_preflight()
    }

    pub fn export_vault(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<VaultExportReport> {
        self.recovery().export_vault(path, dry_run, overwrite)
    }
}
