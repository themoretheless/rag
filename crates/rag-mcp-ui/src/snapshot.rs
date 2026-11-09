//! Snapshot / vault `graph.json` reading and writing (Mode C).
//! Direct DuckDB writes are forbidden; mutations go through the HTTP gateway.

use rag_mcp_core::{GraphEdge, GraphNode, GraphView};
use serde::Deserialize;
#[cfg(feature = "direct-db")]
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::dto::{GraphSourceKind, LoadedGraph};
use crate::topology::UI_HARD_MAX_NODES;

fn projects_from_view(view: &GraphView) -> Vec<String> {
    let mut projects: Vec<_> = view
        .nodes
        .iter()
        .filter_map(|node| serde_json::from_str::<serde_json::Value>(&node.metadata_json).ok())
        .filter_map(|meta| meta.get("wing")?.as_str().map(str::to_owned))
        .filter(|project| !project.trim().is_empty())
        .collect();
    projects.sort();
    projects.dedup();
    projects
}

/// Optional envelope around bare `GraphView` (EGUI_GRAPH_VIEW §10.1).
#[derive(Debug, Deserialize)]
struct SnapshotEnvelope {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    graph: Option<GraphView>,
    #[serde(default)]
    nodes: Option<Vec<GraphNode>>,
    #[serde(default)]
    edges: Option<Vec<GraphEdge>>,
}

/// Load a GraphView JSON snapshot from disk (read-only).
///
/// Accepts bare `GraphView` serde JSON (same shape as MCP `get_graph` / models)
/// or the optional envelope in EGUI_GRAPH_VIEW §10.1.
pub fn load_snapshot_path(path: &Path) -> Result<LoadedGraph, String> {
    if !path.exists() {
        return Err(format!(
            "cannot open snapshot: path does not exist: {}",
            path.display()
        ));
    }
    let bytes =
        fs::read(path).map_err(|e| format!("cannot open snapshot {}: {e}", path.display()))?;
    let view = parse_graph_json(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let raw_node_count = view.nodes.len();
    let truncated = raw_node_count > UI_HARD_MAX_NODES;
    let mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let source = if path.file_name().and_then(|s| s.to_str()) == Some("graph.json") {
        GraphSourceKind::VaultGraphJson {
            path: path.to_path_buf(),
            mtime,
        }
    } else {
        GraphSourceKind::SnapshotFile {
            path: path.to_path_buf(),
            mtime,
        }
    };
    Ok(LoadedGraph {
        projects: Some(projects_from_view(&view)),
        view,
        source,
        truncated,
        raw_node_count,
        health: None,
    })
}

/// Parse bare `GraphView` or envelope `{ "graph": { ... } }`.
///
/// Bare form matches `rag_mcp::models::GraphView` serde (MCP tool wire JSON).
pub fn parse_graph_json(bytes: &[u8]) -> Result<GraphView, String> {
    if let Ok(view) = serde_json::from_slice::<GraphView>(bytes) {
        return Ok(view);
    }
    let env: SnapshotEnvelope = serde_json::from_slice(bytes)
        .map_err(|e| format!("JSON is neither bare GraphView nor snapshot envelope: {e}"))?;
    if let Some(graph) = env.graph {
        return Ok(graph);
    }
    if let (Some(nodes), Some(edges)) = (env.nodes, env.edges) {
        return Ok(GraphView { nodes, edges });
    }
    if env.version.is_some() {
        return Ok(GraphView::default());
    }
    Err("snapshot JSON missing graph / nodes+edges".into())
}

/// Serialize a `GraphView` to the same serde JSON format as `rag_mcp` models / MCP tools.
#[cfg(any(test, feature = "direct-db"))]
pub fn export_graph_view_json(view: &GraphView) -> Result<String, String> {
    serde_json::to_string_pretty(view).map_err(|e| format!("serialize GraphView: {e}"))
}

/// Optional Mode C envelope written by `export --envelope` (EGUI_GRAPH_VIEW §10.1).
#[derive(Debug, Serialize)]
#[cfg(feature = "direct-db")]
pub(crate) struct SnapshotEnvelopeOut<'a> {
    pub(crate) version: u32,
    pub(crate) source: &'static str,
    pub(crate) db_path: String,
    pub(crate) graph: &'a GraphView,
}

/// Write a topology-only GraphView snapshot file (Mode C refresh source).
#[cfg(feature = "direct-db")]
pub fn write_graph_snapshot(path: &Path, view: &GraphView) -> Result<(), String> {
    let json = export_graph_view_json(view)?;
    write_snapshot_bytes(path, json.as_bytes())
}

#[cfg(feature = "direct-db")]
pub(crate) fn write_snapshot_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create snapshot parent {}: {e}", parent.display()))?;
        }
    }
    fs::write(path, bytes).map_err(|e| format!("write snapshot {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::test_support::{edge, node};

    #[test]
    fn parse_bare_graph_view() {
        let json = r#"{"nodes":[{"id":"a","kind":"document","label":"A","document_id":null,"uri":null,"resolved":true,"metadata_json":"{}"}],"edges":[]}"#;
        let v = parse_graph_json(json.as_bytes()).unwrap();
        assert_eq!(v.nodes.len(), 1);
    }

    #[test]
    fn export_roundtrips_models_graph_view() {
        let view = GraphView {
            nodes: vec![node("a", "A")],
            edges: vec![edge("e1", "a", "a")],
        };
        let json = export_graph_view_json(&view).unwrap();
        let back = parse_graph_json(json.as_bytes()).unwrap();
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.nodes[0].id, "a");
        assert_eq!(back.edges[0].rel_type, "wikilink");
    }

    #[test]
    fn parse_envelope_graph() {
        let json = r#"{"version":1,"graph":{"nodes":[{"id":"x","kind":"document","label":"X","document_id":null,"uri":null,"resolved":true,"metadata_json":"{}"}],"edges":[]}}"#;
        let v = parse_graph_json(json.as_bytes()).unwrap();
        assert_eq!(v.nodes[0].id, "x");
    }
}
