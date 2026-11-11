//! Store methods kept for `rag-mcp-ui`'s direct-db reader and its test
//! fixtures, which this crate does not edit. Each forwards to its repository;
//! code in this crate calls the repositories directly.

use super::*;
use crate::models::{GraphEdge, GraphFilter, GraphNode, GraphStats, GraphView};

impl Store {
    pub fn export_graph_for_ui(
        &self,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.graph().export_graph_for_ui(max_nodes, include_tags)
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

    pub fn export_project_graph_for_ui(
        &self,
        project: &str,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.graph()
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
        self.graph()
            .export_project_neighbors_for_ui(project, seed, depth, max_nodes, include_tags)
    }

    pub fn find_by_uri(&self, uri: &str) -> Result<Option<Document>> {
        self.documents().find_by_uri(uri)
    }

    pub fn get_document(&self, id: &str) -> Result<Option<Document>> {
        self.documents().get_document(id)
    }

    pub fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView> {
        self.graph().get_graph_view(filter)
    }

    pub fn graph_stats(&self) -> Result<GraphStats> {
        self.graph().graph_stats()
    }

    pub fn list_projects(&self) -> Result<Vec<crate::models::ProjectSummary>> {
        self.documents().list_projects()
    }

    pub fn list_wiki_page_metas_filtered(
        &self,
        filter: &WikiPageMetaFilter,
    ) -> Result<(Vec<crate::models::WikiPageListItem>, usize)> {
        self.documents().list_wiki_page_metas_filtered(filter)
    }

    pub fn wiki_backlinks_for_document(&self, document_id: &str) -> Result<Vec<(String, String)>> {
        self.documents().wiki_backlinks_for_document(document_id)
    }

    pub fn wiki_backlinks_for_document_in_wing(
        &self,
        document_id: &str,
        wing: &str,
    ) -> Result<Vec<(String, String)>> {
        self.documents()
            .wiki_backlinks_for_document_in_wing(document_id, wing)
    }

    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        self.graph().insert_graph_edges(edges)
    }

    pub fn upsert_document(&self, doc: &Document) -> Result<()> {
        self.documents().upsert_document(doc)
    }

    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()> {
        self.graph().upsert_graph_node(node)
    }
}
