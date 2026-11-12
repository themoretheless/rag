//! Captured PHP name tables and declared-target candidates; no autoload/runtime binding.
use super::*;
mod capture;
mod contexts;
mod hierarchy;
mod phpunit;
mod resolve;
mod traits;
const MAX_SOURCE: usize = 2 * 1024 * 1024;
const MAX_NODES: usize = 100000;
const MAX_USES: usize = 10000;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Import {
    kind: String,
    #[serde(default)]
    start_byte: usize,
    #[serde(default)]
    end_byte: usize,
    alias: String,
    target: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Scope {
    start: usize,
    end: usize,
    name: String,
    imports: Vec<Import>,
    complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Base {
    expression: String,
    line: usize,
    relation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Definition {
    scope: usize,
    #[serde(default)]
    declaration_kind: String,
    id: String,
    fqn: String,
    kind: String,
    owner: Option<String>,
    conditional: bool,
    is_static: bool,
    visibility: String,
    has_body: bool,
    #[serde(default)]
    bases: Vec<Base>,
    #[serde(default)]
    uses_traits: bool,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    trait_uses: Vec<TraitUse>,
    #[serde(default)]
    is_abstract: bool,
    #[serde(default)]
    test_attributes: Vec<String>,
    #[serde(default)]
    test_attributes_complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TraitUse {
    names: Vec<String>,
    rules: Vec<TraitRule>,
    line: usize,
    complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TraitRule {
    source: Option<String>,
    method: String,
    excluded: Vec<String>,
    alias: Option<String>,
    visibility: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Use {
    reference: Reference,
    scope: usize,
    kind: String,
    target: String,
    owner: Option<String>,
    instance: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Facts {
    format: u32,
    complete: bool,
    scopes: Vec<Scope>,
    definitions: Vec<Definition>,
    uses: Vec<Use>,
}
pub(super) fn supported(path: &str) -> bool {
    matches!(path.rsplit('.').next(), Some("php" | "phtml"))
}
pub(super) fn current(doc: &IndexedDocument) -> bool {
    !supported(&doc.path) || doc.php_names.as_ref().is_some_and(|f| f.format == 6)
}
fn children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn class(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration"
    )
}
fn plain(raw: &str) -> Option<String> {
    if raw.len() > 1024 {
        return None;
    }
    let value = raw.trim_start_matches('\\');
    (!value.is_empty()
        && value.split('\\').all(|p| {
            !p.is_empty()
                && p.chars().enumerate().all(|(i, c)| {
                    c == '_'
                        || !c.is_ascii()
                        || if i == 0 {
                            c.is_ascii_alphabetic()
                        } else {
                            c.is_ascii_alphanumeric()
                        }
                })
        }))
    .then(|| value.to_string())
}
fn qualify(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.into()
    } else {
        format!("{namespace}\\{name}")
    }
}
pub(super) fn finish(root: Node, source: &str, id: &str, out: &mut IndexedDocument) {
    capture::finish(root, source, id, out)
}
pub(super) fn document(
    documents: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<CallEdge> {
    resolve::document(documents, doc)
}

#[cfg(test)]
mod tests;

pub(super) fn edges(
    documents: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<super::inheritance::InheritanceEdge> {
    hierarchy::edges(documents, doc)
}
pub(super) fn overrides(
    documents: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<super::inheritance::OverrideEdge> {
    hierarchy::overrides(documents, doc)
}

pub(super) fn context_sites(doc: &IndexedDocument) -> HashSet<(usize, bool, String)> {
    contexts::required_sites(doc)
}

#[derive(Default)]
pub(super) struct ContextIds<'a> {
    pub(super) owners: HashSet<&'a str>,
    pub(super) methods: HashSet<&'a str>,
    pub(super) trait_methods: HashSet<&'a str>,
}
pub(super) fn context_ids<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> ContextIds<'a> {
    contexts::allowed_ids(docs, doc)
}

pub(super) fn test_entrypoints(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<super::analysis::EntryPoint> {
    phpunit::entrypoints(docs, doc)
}
