//! Source-bound Python value-dependency candidates; no runtime/whole-program proof.
use super::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
mod capture;
mod diff;
pub(super) use diff::compare;
mod query;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct NodeFact {
    id: String,
    function: String,
    start_byte: usize,
    end_byte: usize,
    line: usize,
    end_line: usize,
    kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    flow_context: Vec<String>,
    name: Option<String>,
    #[serde(default)]
    binding_name: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct FlowEdge {
    source: String,
    target: String,
    kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Call {
    #[serde(default)]
    binding_qualified: bool,
    start_byte: usize,
    expression: String,
    arguments: Vec<String>,
    result: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Function {
    #[serde(default)]
    target_binding_qualified: bool,
    symbol: String,
    qualified: bool,
    reason: Option<String>,
    parameters: Vec<String>,
    returns: Vec<String>,
    nodes: Vec<NodeFact>,
    edges: Vec<FlowEdge>,
    calls: Vec<Call>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Facts {
    format: u32,
    complete: bool,
    functions: Vec<Function>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentFacts {
    document_id: String,
    path: String,
    project: Option<String>,
    revision: i64,
    content_hash: String,
    facts: Facts,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Snapshot {
    documents: Vec<DocumentFacts>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DataSliceQuery {
    /// Unique exact function ID, qualified name or short name.
    pub symbol: String,
    pub project: Option<String>,
    pub path: Option<String>,
    /// Optional local definition/parameter name; multiple writes require start_byte.
    pub variable: Option<String>,
    /// Exact UTF-8 byte start of the selected definition token.
    pub start_byte: Option<usize>,
    /// back (default), forward or both. Defaults seed returns/back, parameters/forward.
    pub direction: Option<String>,
    pub depth: Option<usize>,
    pub limit: Option<usize>,
    pub max_work: Option<usize>,
}
fn binding_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.nfkc().collect()
}
pub(super) fn current(doc: &IndexedDocument) -> bool {
    !doc.path.ends_with(".py") || doc.data_flow.as_ref().is_some_and(|f| f.format == 7)
}
pub(super) fn capture(root: Node, source: &str, id: &str, symbols: &[Symbol]) -> Facts {
    capture::capture(root, source, id, symbols)
}
pub(super) fn snapshot(docs: &HashMap<String, IndexedDocument>) -> Snapshot {
    let mut documents: Vec<_> = docs
        .iter()
        .filter(|(_, d)| !d.parse_errors)
        .filter_map(|(id, d)| {
            d.data_flow
                .as_ref()
                .filter(|f| f.format == 7)
                .map(|facts| DocumentFacts {
                    document_id: id.clone(),
                    path: d.path.clone(),
                    project: d.project.clone(),
                    revision: d.revision,
                    content_hash: d.hash.clone(),
                    facts: facts.clone(),
                })
        })
        .collect();
    documents.sort_by(|a, b| a.document_id.cmp(&b.document_id));
    Snapshot { documents }
}
impl Store {
    pub fn code_data_slice(&self, q: &DataSliceQuery) -> Result<Value> {
        if q.start_byte.is_some() && q.variable.is_none() {
            return Err(AppError::config("start_byte requires variable"));
        }
        if q.symbol.is_empty()
            || !matches!(
                q.direction.as_deref().unwrap_or("back"),
                "back" | "forward" | "both"
            )
        {
            return Err(AppError::config(
                "symbol is required; direction must be back, forward or both",
            ));
        }
        let r = self.read()?;
        let snapshot = super::snapshot_locked(&*r, q.project.as_deref())?;
        Ok(query::query(&snapshot, q))
    }
}

#[cfg(test)]
mod except_tests;
#[cfg(test)]
mod finally_tests;
#[cfg(test)]
mod tests;
