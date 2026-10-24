//! Versioned syntax index over ingested source documents, independent of the wiki graph.
//! Static bindings are conservative; dynamic dispatch and unresolved types remain explicit.
mod analysis;
mod c_family;
pub mod corpus;
pub mod drift;
mod go;
pub mod history;
mod inheritance;
mod java;
pub mod policy;
mod references;
mod repo_map;
mod resolve;
mod shell;
pub(crate) mod source;
pub mod sql;
pub mod styles;
pub(crate) const INDEX_FORMAT_VERSION: u32 = 20;
use crate::{
    db::Store,
    error::{AppError, Result},
    util::content_hash,
};
use duckdb::params;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: String,
    pub document_id: String,
    pub path: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub signature: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub revision: i64,
    pub content_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    #[serde(default)]
    pub start_byte: usize,
    pub document_id: String,
    pub path: String,
    pub name: String,
    pub expression: String,
    pub line: usize,
    pub column: usize,
    pub caller: Option<String>,
    pub is_call: bool,
    pub revision: i64,
    pub content_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedDocument {
    version: u32,
    hash: String,
    revision: i64,
    path: String,
    symbols: Vec<Symbol>,
    references: Vec<Reference>,
    parse_errors: bool,
    #[serde(default)]
    bindings: Vec<resolve::Binding>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    exports: HashMap<String, String>,
    #[serde(default)]
    bases: Vec<inheritance::BaseSpec>,
    #[serde(default)]
    routes: Vec<analysis::RouteSpec>,
    #[serde(default)]
    blocked_members: HashSet<String>,
    #[serde(default)]
    test_definitions: HashSet<usize>,
    #[serde(default)]
    sql: sql::SqlDocument,
    #[serde(default)]
    styles: styles::StyleDocument,
    #[serde(default)]
    policies: Vec<policy::PolicySpec>,
    #[serde(default)]
    binding_limitations: Vec<String>,
    #[serde(default)]
    go: Option<go::Facts>,
    #[serde(default)]
    java: Option<java::Facts>,
    #[serde(default)]
    c_family: Option<c_family::Facts>,
    #[serde(default)]
    shell: Option<shell::Facts>,
}
pub use rag_mcp_core::IndexReport;
#[derive(Debug, Clone, Serialize)]
pub struct CallEdge {
    pub reference: Reference,
    pub candidates: Vec<String>,
    pub resolution: String,
    pub provenance: &'static str,
    pub resolution_basis: String,
}
#[derive(Debug, Default, Serialize)]
pub struct CodeSnapshot {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    pub calls: Vec<CallEdge>,
    pub bound_references: Vec<CallEdge>,
    pub inheritance: Vec<inheritance::InheritanceEdge>,
    pub entrypoints: Vec<analysis::EntryPoint>,
    pub sql: sql::SqlSnapshot,
    pub styles: styles::StyleSnapshot,
    #[serde(skip)]
    policies: Vec<policy::PolicySpec>,
    #[serde(skip)]
    policy_projects: HashMap<String, Option<String>>,
    pub stale_documents: Vec<String>,
    pub unindexed_documents: Vec<String>,
    pub parse_errors: Vec<String>,
    pub index_revision: Option<i64>,
    pub binding_diagnostics: Vec<serde_json::Value>,
}
fn is_sql(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
}
fn indexable(path: &str) -> bool {
    go::manifest(path) || is_sql(path) || is_markup(path) || language(path).is_some()
}
fn is_markup(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "css" | "html" | "htm" | "vue" | "svelte"
    )
}
fn language(path: &str) -> Option<tree_sitter::Language> {
    Some(match path.rsplit('.').next()? {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "c" | "h" => tree_sitter_c::LANGUAGE.into(),
        "C" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => tree_sitter_cpp::LANGUAGE.into(),
        "sh" | "bash" => tree_sitter_bash::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "php" | "phtml" => tree_sitter_php::LANGUAGE_PHP.into(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => return None,
    })
}
fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
fn definition(node: Node) -> bool {
    if node.kind() == "variable_declarator" {
        return node.child_by_field_name("value").is_some_and(|v| {
            matches!(
                v.kind(),
                "arrow_function" | "function_expression" | "generator_function"
            )
        });
    }
    matches!(
        node.kind(),
        "function_item"
            | "function_signature_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "mod_item"
            | "type_item"
            | "function_definition"
            | "class_definition"
            | "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "method_definition"
            | "method_declaration"
            | "method_signature"
            | "interface_declaration"
            | "type_alias_declaration"
            | "type_spec"
            | "type_alias"
            | "method_elem"
            | "constructor_declaration"
            | "compact_constructor_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration"
    )
}
fn last_identifier(node: Node) -> Option<Node> {
    // Generic type arguments and receiver expressions do not name the callee.
    for field in ["field", "attribute", "property", "name", "function"] {
        if let Some(child) = node.child_by_field_name(field) {
            if let Some(name) = last_identifier(child) {
                return Some(name);
            }
        }
    }
    if matches!(
        node.kind(),
        "identifier" | "field_identifier" | "property_identifier" | "type_identifier" | "name"
    ) {
        return Some(node);
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    children.into_iter().rev().find_map(last_identifier)
}
fn parse(id: &str, path: &str, source: &str, revision: i64) -> Result<IndexedDocument> {
    let mut out = IndexedDocument {
        version: INDEX_FORMAT_VERSION,
        hash: content_hash(source),
        revision,
        path: path.into(),
        symbols: vec![],
        references: vec![],
        parse_errors: false,
        bindings: vec![],
        project: None,
        exports: HashMap::new(),
        bases: vec![],
        routes: vec![],
        blocked_members: HashSet::new(),
        test_definitions: HashSet::new(),
        sql: sql::SqlDocument::default(),
        styles: styles::StyleDocument::default(),
        policies: vec![],
        binding_limitations: vec![],
        go: None,
        java: None,
        c_family: None,
        shell: None,
    };
    if go::manifest(path) {
        go::parse_manifest(source, &mut out);
        return Ok(out);
    }
    if path.ends_with(".go") {
        out.go = Some(go::Facts::default());
    }
    if path.ends_with(".java") {
        out.java = Some(java::Facts::default());
    }
    if c_family::supported(path) {
        out.c_family = Some(c_family::Facts::new(path));
    }
    if matches!(path.rsplit('.').next(), Some("sh" | "bash")) {
        out.shell = Some(shell::Facts::default());
    }
    if is_markup(path) {
        styles::parse_markup(source, &mut out)?;
        return Ok(out);
    }
    if is_sql(path) {
        sql::parse_raw(source, &mut out);
        return Ok(out);
    }
    let mut parser = Parser::new();
    parser
        .set_language(
            &language(path).ok_or_else(|| AppError::config("unsupported source language"))?,
        )
        .map_err(|e| AppError::config(e.to_string()))?;
    let mut tree = parser
        .parse(source, None)
        .ok_or_else(|| AppError::config("source parse cancelled"))?;
    if path.ends_with(".h") && tree.root_node().has_error() {
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .map_err(|e| AppError::config(e.to_string()))?;
        if let Some(alternate) = parser.parse(source, None) {
            if !alternate.root_node().has_error() {
                tree = alternate;
                out.c_family.as_mut().unwrap().cpp = true;
            }
        }
    }
    out.parse_errors = tree.root_node().has_error();
    walk(
        tree.root_node(),
        source,
        id,
        &mut out,
        WalkContext {
            scope: &[],
            caller: None,
            in_call_target: false,
            depth: 0,
        },
    );
    sql::finish(source, &mut out);
    policy::finish(tree.root_node(), source, &mut out);
    // Declaration names are not usages.
    Ok(out)
}
struct WalkContext<'a> {
    scope: &'a [String],
    caller: Option<String>,
    in_call_target: bool,
    depth: usize,
}
fn walk(node: Node, source: &str, doc: &str, out: &mut IndexedDocument, context: WalkContext<'_>) {
    let WalkContext {
        scope,
        caller,
        in_call_target,
        depth,
    } = context;
    if depth >= 256 {
        // Malformed/adversarial trees must not exhaust the process stack.
        out.parse_errors = true;
        return;
    }
    if matches!(out.path.rsplit('.').next(), Some("php" | "phtml"))
        && matches!(
            node.kind(),
            "namespace_definition" | "namespace_use_declaration"
        )
        && out.binding_limitations.is_empty()
    {
        out.binding_limitations
            .push("PHP_NAMESPACE_BINDING_UNMODELED".into());
    }
    let c_owner = if out.shell.is_some() {
        shell::collect(node, source, doc, out)
    } else if out.c_family.is_some() {
        c_family::collect(node, source, doc, out)
    } else {
        None
    };
    if out.c_family.is_some() || out.shell.is_some() {
        // Language-specific declarators and namespaces are collected above.
    } else if out.java.is_some() {
        java::collect(node, source, out);
    } else if out.go.is_some() {
        go::collect(node, source, out);
    } else {
        resolve::collect(node, source, out);
    }
    inheritance::collect(node, source, out);
    if out.c_family.is_none() && out.shell.is_none() {
        analysis::collect(node, source, out);
    }
    styles::collect(node, source, out);
    let mut nested = scope.to_vec();
    let mut owner = c_owner.or(caller);
    let name_node = if out.c_family.is_none() && out.shell.is_none() && definition(node) {
        node.child_by_field_name("name")
    } else {
        None
    };
    if let Some(name) = name_node {
        let name = text(name, source).to_string();
        if out.path.ends_with(".go") && node.kind() == "method_declaration" {
            nested.push(
                go::receiver_type(node, source)
                    .unwrap_or_else(|| format!("<receiver@{}>", node.start_byte())),
            );
        }
        nested.push(name.clone());
        let id = format!("{}:{}:{}", doc, node.start_byte(), name);
        let body = node.child_by_field_name("body").or_else(|| {
            node.child_by_field_name("value")
                .and_then(|v| v.child_by_field_name("body"))
        });
        out.symbols.push(Symbol {
            id: id.clone(),
            document_id: doc.into(),
            path: out.path.clone(),
            name,
            qualified_name: nested.join("::"),
            kind: node.kind().into(),
            signature: source[node.start_byte()..body.map_or(node.end_byte(), |b| b.start_byte())]
                .trim()
                .into(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            revision: out.revision,
            content_hash: out.hash.clone(),
        });
        owner = Some(id);
    }
    if out.java.is_some() && java::anonymous_body(node) {
        nested.push(format!("<anonymous@{}>", node.start_byte()));
    }
    // Rust impl blocks provide a type scope, including methods with identical names.
    if node.kind() == "impl_item" {
        if let Some(ty) = node.child_by_field_name("type") {
            nested.push(text(ty, source).into());
        }
    }
    sql::collect(node, source, out, owner.as_deref());
    let target = if matches!(
        node.kind(),
        "call_expression" | "call" | "new_expression" | "function_call_expression"
    ) {
        node.child_by_field_name("function")
            .or_else(|| node.child_by_field_name("constructor"))
    } else {
        None
    };
    if out.shell.is_some() {
        shell::push_references(node, source, doc, out, owner.as_deref());
    }
    if out.c_family.is_some() {
        c_family::push_new(node, source, doc, out, owner.as_deref());
    }
    if out.java.is_some() {
        java::push_call(node, source, doc, out, owner.as_deref());
    }
    if let Some(target) = target {
        if let Some(name) = if out.c_family.is_some() {
            c_family::terminal_name(target)
        } else {
            last_identifier(target)
        } {
            out.references.push(Reference {
                start_byte: node.start_byte(),
                document_id: doc.into(),
                path: out.path.clone(),
                name: text(name, source).into(),
                expression: text(target, source).into(),
                line: name.start_position().row + 1,
                column: name.start_position().column + 1,
                caller: owner.clone(),
                is_call: true,
                revision: out.revision,
                content_hash: out.hash.clone(),
            });
        }
    }
    if !in_call_target && name_node.is_none() {
        if let Some((name, expression)) = references::occurrence(node, source, out) {
            out.references.push(Reference {
                start_byte: node.start_byte(),
                document_id: doc.into(),
                path: out.path.clone(),
                name: text(name, source).into(),
                expression,
                line: name.start_position().row + 1,
                column: name.start_position().column + 1,
                caller: owner.clone(),
                is_call: false,
                revision: out.revision,
                content_hash: out.hash.clone(),
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if name_node.is_some_and(|n| n.id() == child.id()) {
            continue;
        }
        walk(
            child,
            source,
            doc,
            out,
            WalkContext {
                scope: &nested,
                caller: owner.clone(),
                in_call_target: in_call_target || target.is_some_and(|n| n.id() == child.id()),
                depth: depth + 1,
            },
        );
    }
}
impl Store {
    /// Explicit rebuild of derived syntax data. Never opens or reads source paths.
    /// Source text and cached payloads are read under one database lock.
    pub fn index_code(&self) -> Result<IndexReport> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let docs = {
            let mut stmt = tx.prepare("SELECT id, COALESCE(source_file, uri), content, COALESCE(revision,1), wing, metadata_json FROM documents WHERE COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated') ORDER BY id")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?;
            let raw = rows.collect::<std::result::Result<Vec<_>, _>>()?;
            raw.into_iter()
                .map(|(id, path, content, rev, project, metadata)| {
                    let text = source::read(&tx, &content, &metadata)?.into_owned();
                    Ok((id, path, text, rev, project))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let mut report = IndexReport::default();
        // Replace the complete corpus snapshot atomically; rollback preserves the old one on errors.
        let mut payloads = Vec::new();
        for (id, path, source, revision, project) in &docs {
            if !indexable(path) {
                report.unsupported += 1;
                continue;
            }
            let cached: Option<String> = tx
                .query_row(
                    "SELECT payload FROM code_index_documents WHERE document_id=?",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            let parsed = cached
                .as_deref()
                .and_then(|s| serde_json::from_str::<IndexedDocument>(s).ok())
                .filter(|d| {
                    d.version == INDEX_FORMAT_VERSION
                        && d.hash == content_hash(source)
                        && d.revision == *revision
                        && d.path == *path
                        && d.project == *project
                });
            let mut parsed = match parsed {
                Some(p) => {
                    report.unchanged += 1;
                    p
                }
                None => {
                    report.indexed += 1;
                    parse(id, path, source, *revision)?
                }
            };
            parsed.project = project.clone();
            if parsed.parse_errors {
                report.parse_errors.push(id.clone());
            }
            payloads.push((id.clone(), serde_json::to_string(&parsed)?));
        }
        tx.execute("DELETE FROM code_index_documents", [])?;
        for (id, payload) in &payloads {
            tx.execute(
                "INSERT INTO code_index_documents VALUES (?,?)",
                params![id, payload],
            )?;
        }
        let (revision, created) = history::publish(&tx, &docs, &payloads)?;
        report.index_revision = Some(revision);
        report.revision_created = created;
        tx.commit()?;
        Ok(report)
    }
    pub fn code_body(&self, q: &CodeQuery) -> Result<serde_json::Value> {
        if q.symbol.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Err(AppError::config("code requires a symbol name or ID"));
        }
        let snapshot = self.code_snapshot()?;
        let matches: Vec<_> = snapshot
            .symbols
            .iter()
            .filter(|s| {
                q.path.as_ref().is_none_or(|p| p == &s.path)
                    && q.symbol
                        .as_ref()
                        .is_some_and(|n| n == &s.name || n == &s.id || n == &s.qualified_name)
            })
            .collect();
        if matches.len() != 1 {
            return Ok(
                serde_json::json!({"found":false,"index_rev":snapshot.index_revision,"ambiguous":matches.len()>1,
                "candidates":matches,"stale_documents":snapshot.stale_documents,
                "unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors}),
            );
        }
        let symbol = matches[0];
        let document = self
            .get_document(&symbol.document_id)?
            .ok_or_else(|| AppError::not_found("source was deleted"))?;
        let connection = self.lock()?;
        let original = source::read(&connection, &document.content, &document.metadata_json)?;
        if document.revision != symbol.revision
            || content_hash(&original) != symbol.content_hash
            || matches!(
                document.status.as_str(),
                "archived" | "tombstone" | "consolidated"
            )
            || document.source_file.as_deref().unwrap_or(&document.uri) != symbol.path
        {
            return Err(AppError::Conflict(
                "source changed during code lookup; refresh index_code".into(),
            ));
        }
        let body = original
            .get(symbol.start_byte..symbol.end_byte)
            .ok_or_else(|| AppError::config("invalid cached symbol span; rebuild index_code"))?;
        Ok(
            serde_json::json!({"found":true,"index_rev":snapshot.index_revision,"symbol":symbol,"code":body,
            "parse_errors":snapshot.parse_errors}),
        )
    }

    /// Stale facts are omitted; their document IDs are explicitly reported.
    pub fn code_snapshot(&self) -> Result<CodeSnapshot> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT d.id, COALESCE(d.source_file,d.uri), d.content, COALESCE(d.revision,1), c.payload, d.wing, d.metadata_json FROM documents d LEFT JOIN code_index_documents c ON c.document_id=d.id WHERE COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') ORDER BY d.id")?;
        let mut rows = stmt.query([])?;
        let mut out = CodeSnapshot {
            index_revision: conn.query_row(
                "SELECT MAX(id) FROM code_index_revisions",
                [],
                |r| r.get(0),
            )?,
            ..CodeSnapshot::default()
        };
        let mut documents = HashMap::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let path: String = row.get(1)?;
            if !indexable(&path) {
                continue;
            }
            let content: String = row.get(2)?;
            let metadata: String = row.get(6)?;
            let source = source::read(&conn, &content, &metadata)?;
            let rev: i64 = row.get(3)?;
            let payload: Option<String> = row.get(4)?;
            let Some(payload) = payload else {
                if go::manifest(&path) {
                    let mut boundary = parse(&id, &path, "", rev)?;
                    boundary.project = row.get(5)?;
                    documents.insert(id.clone(), boundary);
                    out.binding_diagnostics.push(serde_json::json!({"document_id":id,"path":path,"code":"GO_MANIFEST_BOUNDARY_UNVERIFIED"}));
                }
                out.unindexed_documents.push(id);
                continue;
            };
            let cached: IndexedDocument = serde_json::from_str(&payload)?;
            if cached.version != INDEX_FORMAT_VERSION
                || cached.hash != content_hash(&source)
                || cached.path != path
                || cached.revision != rev
                || cached.project != row.get::<_, Option<String>>(5)?
            {
                if go::manifest(&path) {
                    let mut boundary = parse(&id, &path, "", rev)?;
                    boundary.project = row.get(5)?;
                    documents.insert(id.clone(), boundary);
                    out.binding_diagnostics.push(serde_json::json!({"document_id":id,"path":path,"code":"GO_MANIFEST_BOUNDARY_UNVERIFIED"}));
                }
                out.stale_documents.push(id);
                continue;
            }
            if cached.parse_errors {
                out.parse_errors.push(id.clone());
            }
            for limitation in &cached.binding_limitations {
                out.binding_diagnostics
                    .push(serde_json::json!({"document_id":id,"path":path,"code":limitation}));
            }
            out.policies.extend(cached.policies.clone());
            out.policy_projects
                .insert(id.clone(), cached.project.clone());
            out.symbols.extend(cached.symbols.clone());
            out.references.extend(cached.references.clone());
            documents.insert(row.get::<_, String>(0)?, cached);
        }
        out.binding_diagnostics
            .extend(shell::diagnostics(&documents));
        out.sql = sql::snapshot(&documents);
        out.styles = styles::snapshot(&documents);
        out.entrypoints = analysis::entrypoints(&out.symbols, &documents);
        out.inheritance = inheritance::edges(&out.symbols, &documents);
        out.bound_references = resolve::references(&out.symbols, &documents);
        out.calls = out
            .bound_references
            .iter()
            .filter(|r| r.reference.is_call)
            .cloned()
            .collect();
        Ok(out)
    }
}
use duckdb::OptionalExt;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CodeQuery {
    /// Exact symbol ID, qualified name, or short name. Omit to list symbols.
    pub symbol: Option<String>,
    /// Exact ingested source path. Does not read the filesystem.
    pub path: Option<String>,
    /// Maximum returned records, clamped to 1..500.
    pub limit: Option<usize>,
    /// Caller traversal depth for impact (1..10, default 3).
    pub depth: Option<usize>,
    /// Structural slice traversal: back (default), forward or both.
    pub direction: Option<String>,
    /// Repository map character budget (128..20000, default 4000).
    pub budget_chars: Option<usize>,
    /// References: bound (default) or lexical name matches, including unresolved names.
    pub reference_mode: Option<String>,
}
impl CodeSnapshot {
    pub fn query(&self, operation: &str, q: &CodeQuery) -> serde_json::Value {
        let mut result = self.query_inner(operation, q);
        if let Some(object) = result.as_object_mut() {
            object.insert(
                "binding_diagnostics".into(),
                serde_json::json!(self.binding_diagnostics),
            );
            object.insert("index_rev".into(), serde_json::json!(self.index_revision));
            object.insert(
                "view".into(),
                serde_json::json!("fresh_subset_of_revision_inputs"),
            );
        }
        result
    }
    fn query_inner(&self, operation: &str, q: &CodeQuery) -> serde_json::Value {
        if let Some(result) = self.analysis_query(operation, q) {
            return result;
        }
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        let selected: Vec<_> = self
            .symbols
            .iter()
            .filter(|s| {
                q.path.as_ref().is_none_or(|p| p == &s.path)
                    && q.symbol
                        .as_ref()
                        .is_none_or(|n| n == &s.name || n == &s.id || n == &s.qualified_name)
            })
            .collect();
        let ids: HashSet<_> = selected.iter().map(|s| s.id.as_str()).collect();
        if operation == "references" {
            return references::query(self, q, &selected, limit);
        }
        if operation == "inheritance" {
            let edges: Vec<_> = self
                .inheritance
                .iter()
                .filter(|e| ids.contains(e.derived.as_str()))
                .collect();
            return serde_json::json!({"items":edges.iter().take(limit).collect::<Vec<_>>(),"total":edges.len(),"truncated":edges.len()>limit,
                "stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors});
        }
        if operation == "repo_map" {
            return repo_map::query(self, &selected, q, limit);
        }
        if operation == "impact" {
            let mut visited: HashSet<String> = ids.iter().map(|id| (*id).to_string()).collect();
            let mut frontier = visited.clone();
            let mut affected = Vec::new();
            let mut truncated = false;
            for depth in 1..=q.depth.unwrap_or(3).clamp(1, 10) {
                let mut next = HashSet::new();
                for call in &self.calls {
                    if !call.candidates.iter().any(|id| frontier.contains(id)) {
                        continue;
                    }
                    let Some(caller) = &call.reference.caller else {
                        continue;
                    };
                    if !visited.insert(caller.clone()) {
                        continue;
                    }
                    if affected.len() >= limit {
                        truncated = true;
                        continue;
                    }
                    next.insert(caller.clone());
                    affected.push(serde_json::json!({"symbol_id":caller,"depth":depth,"via":call}));
                }
                if next.is_empty() {
                    break;
                }
                frontier = next;
            }
            let depth_limited = self.calls.iter().any(|c| {
                c.candidates.iter().any(|id| frontier.contains(id))
                    && c.reference
                        .caller
                        .as_ref()
                        .is_some_and(|id| !visited.contains(id))
            });
            return serde_json::json!({"items":affected,"truncated":truncated,"depth_limited":depth_limited,
                "matched_symbols":selected.len(),"resolution":"conservative_static_bindings",
                "stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,
                "parse_errors":self.parse_errors});
        }
        let all: Vec<serde_json::Value> = match operation {
            "symbol" => selected
                .iter()
                .map(|s| serde_json::to_value(s).unwrap())
                .collect(),
            "callers" => self
                .calls
                .iter()
                .filter(|c| c.candidates.iter().any(|id| ids.contains(id.as_str())))
                .map(|c| serde_json::to_value(c).unwrap())
                .collect(),
            "callees" => self
                .calls
                .iter()
                .filter(|c| {
                    c.reference
                        .caller
                        .as_deref()
                        .is_some_and(|id| ids.contains(id))
                })
                .map(|c| serde_json::to_value(c).unwrap())
                .collect(),
            _ => vec![],
        };
        serde_json::json!({"items":all.iter().take(limit).collect::<Vec<_>>(), "total":all.len(), "truncated":all.len()>limit,
            "matched_symbols": selected.len(), "stale_documents":self.stale_documents,
            "unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors,
            "resolution":"conservative_static_bindings"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn doc(id: &str, path: &str, code: &str) -> Document {
        Document {
            id: id.into(),
            uri: format!("file:///{path}"),
            source_file: Some(path.into()),
            content: code.into(),
            title: path.into(),
            ..Default::default()
        }
    }
    #[test]
    fn code_index_parses_supported_languages_without_comment_calls() {
        for (path, source) in [
            (
                "a.rs",
                "fn alpha() { beta(); /* ghost(); */ let s = \"fake()\"; } fn beta() {}",
            ),
            (
                "a.py",
                "def alpha():\n    beta()\n    # ghost()\ndef beta():\n    pass\n",
            ),
            (
                "a.js",
                "function alpha() { beta(); /* ghost() */ } function beta() {}",
            ),
            (
                "a.ts",
                "function alpha(): void { beta(); } function beta(): void {}",
            ),
            (
                "a.tsx",
                "function alpha() { beta(); return <div/>; } function beta() {}",
            ),
        ] {
            let parsed = parse("d", path, source, 3).unwrap();
            assert!(!parsed.parse_errors, "{path}");
            assert_eq!(parsed.symbols.len(), 2, "{path}");
            let calls: Vec<_> = parsed.references.iter().filter(|r| r.is_call).collect();
            assert_eq!(calls.len(), 1, "{path}");
            assert_eq!(calls[0].name, "beta");
            assert_eq!(calls[0].caller, Some(parsed.symbols[0].id.clone()));
            assert!(parsed.symbols.iter().all(|s| s.revision == 3));
        }
    }
    #[test]
    fn code_index_nested_scopes_and_method_receivers() {
        let parsed = parse(
            "d",
            "x.rs",
            "struct A; impl A { fn run(&self) { self.work(); } fn work(&self) {} }",
            1,
        )
        .unwrap();
        assert!(parsed.symbols.iter().any(|s| s.qualified_name == "A::run"));
        let call = parsed.references.iter().find(|r| r.is_call).unwrap();
        assert_eq!(call.expression, "self.work");
        assert_eq!(call.name, "work");
    }
    #[test]
    fn code_index_persists_reuses_and_omits_stale_deleted_and_archived_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.duckdb");
        let store = Store::open(&path).unwrap();
        store
            .upsert_document(&doc(
                "a",
                "a.rs",
                "fn alpha() { beta(); external(); } fn beta() {}",
            ))
            .unwrap();
        store
            .upsert_document(&doc("b", "b.rs", "fn beta() {}"))
            .unwrap();
        assert_eq!(store.code_snapshot().unwrap().unindexed_documents.len(), 2);
        assert_eq!(store.index_code().unwrap().indexed, 2);
        assert_eq!(store.index_code().unwrap().unchanged, 2);
        let snap = store.code_snapshot().unwrap();
        assert_eq!(snap.calls[0].resolution, "resolved_static");
        assert_eq!(snap.calls[0].resolution_basis, "lexical");
        assert_eq!(snap.calls[0].candidates.len(), 1);
        assert_eq!(snap.calls[1].resolution, "unresolved");
        let q = CodeQuery {
            symbol: Some("beta".into()),
            limit: Some(1),
            ..Default::default()
        };
        assert_eq!(snap.query("symbol", &q)["truncated"], true);
        assert_eq!(snap.query("callers", &q)["total"], 1);
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.code_snapshot().unwrap().symbols.len(), 3);
        // A source mutation cannot silently return old indexed facts.
        let mut a = store.get_document("a").unwrap().unwrap();
        a.content = "fn replacement() {}".into();
        store.upsert_document(&a).unwrap();
        let snap = store.code_snapshot().unwrap();
        assert_eq!(snap.stale_documents, vec!["a"]);
        assert!(snap.calls.is_empty());
        store.delete_document("b").unwrap();
        store.index_code().unwrap();
        assert_eq!(
            store.code_snapshot().unwrap().symbols[0].name,
            "replacement"
        );
        let mut a = store.get_document("a").unwrap().unwrap();
        a.status = "archived".into();
        store.upsert_document(&a).unwrap();
        assert!(store.code_snapshot().unwrap().symbols.is_empty());
    }
    #[test]
    fn code_index_arrows_generics_and_call_target_arguments() {
        let parsed = parse(
            "d",
            "x.ts",
            "const arrow = () => helper<string>(); function helper<T>() {}",
            1,
        )
        .unwrap();
        assert!(!parsed.parse_errors);
        assert!(parsed.symbols.iter().any(|s| s.name == "arrow"));
        let call = parsed.references.iter().find(|r| r.is_call).unwrap();
        assert_eq!(call.name, "helper");
        assert!(call.caller.as_ref().unwrap().ends_with(":arrow"));
        let parsed = parse(
            "d",
            "x.rs",
            "fn run() { helper::<String>(); } fn helper<T>() {}",
            1,
        )
        .unwrap();
        assert_eq!(
            parsed.references.iter().find(|r| r.is_call).unwrap().name,
            "helper"
        );
    }
    #[test]
    fn code_index_body_map_and_impact_are_bounded_and_honest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
        store
            .upsert_document(&doc(
                "a",
                "a.rs",
                "fn a() { b(); } fn b() { c(); } fn c() { a(); }",
            ))
            .unwrap();
        store
            .upsert_document(&doc("other", "other.rs", "fn c() {}"))
            .unwrap();
        store.index_code().unwrap();
        let snapshot = store.code_snapshot().unwrap();
        let q = CodeQuery {
            symbol: Some("c".into()),
            path: Some("a.rs".into()),
            ..Default::default()
        };
        let body = store.code_body(&q).unwrap();
        assert_eq!(body["code"], "fn c() { a(); }");
        let mut q = CodeQuery {
            symbol: Some("c".into()),
            ..Default::default()
        };
        assert_eq!(store.code_body(&q).unwrap()["ambiguous"], true);
        assert!(store.code_body(&CodeQuery::default()).is_err());
        q.path = Some("a.rs".into());
        let impact = snapshot.query("impact", &q);
        assert_eq!(impact["items"].as_array().unwrap().len(), 2);
        assert_eq!(impact["depth_limited"], false);
        q.depth = Some(1);
        assert_eq!(snapshot.query("impact", &q)["depth_limited"], true);
        q.depth = Some(10);
        q.limit = Some(1);
        assert_eq!(snapshot.query("impact", &q)["truncated"], true);
        let map = snapshot.query(
            "repo_map",
            &CodeQuery {
                budget_chars: Some(128),
                limit: Some(1),
                ..Default::default()
            },
        );
        assert!(map["map"].as_str().unwrap().chars().count() <= 128);
        assert_eq!(map["truncated"], true);
        // Different symbols called c are never silently chosen by code reads.
        assert!(body["symbol"]["content_hash"].is_string());
    }
    #[test]
    fn code_index_import_aliases_and_typed_receivers() {
        for (main_path, main, lib_path, library) in [
            ("/p/src/main.rs", "use crate::a::{go as run, Worker}; fn main(worker: Worker) { run(); worker.work(); }", "/p/src/a.rs", "pub fn go() {} pub struct Worker; impl Worker { pub fn work(&self) {} }"),
            ("/p/main.ts", "import { go as run, Worker } from './a'; function main(worker: Worker) { run(); worker.work(); }", "/p/a.ts", "export function go() {} export class Worker { work() {} }"),
            ("/p/main.py", "from a import go as run, Worker\ndef main(worker: Worker):\n    run()\n    worker.work()\n", "/p/a.py", "def go():\n    pass\nclass Worker:\n    def work(self):\n        pass\n"),
        ] {
            let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("code.duckdb")).unwrap();
            store.upsert_document(&doc("main",main_path,main)).unwrap();
            store.upsert_document(&doc("lib",lib_path,library)).unwrap();
            store.upsert_document(&doc("decoy","/other/decoy.rs","fn go() {} struct Worker; impl Worker { fn work() {} }")).unwrap();
            store.index_code().unwrap();let snapshot=store.code_snapshot().unwrap();
            let calls:Vec<_>=snapshot.calls.iter().filter(|c|c.reference.document_id=="main").collect();
            assert_eq!(calls.len(),2,"{main_path}: {snapshot:?}");
            for call in calls {
                assert_eq!(call.resolution,"resolved_static","{main_path}: {call:?}");
                assert_eq!(call.candidates.len(),1);
                assert!(call.candidates[0].starts_with("lib:"),"{main_path}: {call:?}");
            }
        }
    }
    #[test]
    fn code_index_shadowed_unknown_receivers_do_not_match_unrelated_methods() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
        store
            .upsert_document(&doc(
                "main",
                "/p/main.ts",
                "import {go} from './a'; function main(go, obj) { go(); obj.work(); }",
            ))
            .unwrap();
        store
            .upsert_document(&doc(
                "lib",
                "/p/a.ts",
                "export function go() {} export class Worker { work() {} }",
            ))
            .unwrap();
        store.index_code().unwrap();
        for c in store.code_snapshot().unwrap().calls {
            assert!(c.candidates.is_empty(), "{c:?}");
        }
    }
    #[test]
    fn code_index_namespace_default_exports_self_and_type_aliases() {
        for (main_path,main,lib_path,library) in [
            ("/p/main.ts", "import Default, { go as run } from './a'; import * as api from './a'; type Alias = Default; function main(w: Alias) { run(); api.go(); w.work(); }", "/p/a.ts", "export default class Worker { work() {} } export function go() {}"),
            ("/p/main.js", "import Worker from './a'; import * as api from './a'; const arrow=() => api.go(); function main() { const w=new Worker(); w.work(); arrow(); }", "/p/a.js", "class Worker { work() {} } export {Worker as default}; export function go() {}"),
            ("/p/main.py", "import a as api\ndef main():\n    w=api.Worker()\n    w.work()\n    api.go()\nclass Local:\n    def main(self):\n        self.work()\n    def work(self):\n        pass\n", "/p/a.py", "def go():\n    pass\nclass Worker:\n    def work(self):\n        pass\n"),
            ("/p/src/sub/main.rs", "use crate::a as api; use crate::a::Worker; type Alias = Worker; fn main(w: Alias) { api::go(); w.work(); }", "/p/src/a.rs", "pub fn go() {} pub struct Worker; impl Worker { pub fn work(&self) {} }"),
        ] {
            let dir=tempfile::tempdir().unwrap(); let store=Store::open(&dir.path().join("code.duckdb")).unwrap();
            store.upsert_document(&doc("main",main_path,main)).unwrap();store.upsert_document(&doc("lib",lib_path,library)).unwrap();
            store.index_code().unwrap(); let snap=store.code_snapshot().unwrap();
            assert!(snap.parse_errors.is_empty(),"{main_path}: {snap:?}");
            for c in &snap.calls {assert_eq!(c.resolution,"resolved_static","{main_path}: {c:?}");assert_eq!(c.candidates.len(),1);}
            assert!(!snap.calls.is_empty());
        }
    }
    #[test]
    fn code_index_rust_inline_modules_and_super_imports() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
        store
            .upsert_document(&doc(
                "root",
                "/p/src/lib.rs",
                "pub mod inline { pub fn run() {} } pub fn root() {}",
            ))
            .unwrap();
        store.upsert_document(&doc("caller","/p/src/child.rs","use crate::inline::run as alias; use super::root; fn caller() { alias(); root(); }" )).unwrap();
        store.index_code().unwrap();
        for c in store.code_snapshot().unwrap().calls {
            assert_eq!(c.resolution, "resolved_static", "{c:?}");
            assert!(c.candidates[0].starts_with("root:"));
        }
    }
    #[test]
    fn code_index_external_imports_and_bare_methods_do_not_gain_false_edges() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
        store.upsert_document(&doc("main","/p/main.ts","import { go } from 'external'; import {hidden} from './a'; class Local { run() { work(); go(); hidden(); this.inner.work(); } work() {} } function shadowed() { hidden(); const hidden=()=>{}; } function nested(w: Local) { w.inner.work(); }" )).unwrap();
        store
            .upsert_document(&doc(
                "lib",
                "/p/a.ts",
                "export function go() {} function hidden() {}",
            ))
            .unwrap();
        store.index_code().unwrap();
        for c in store.code_snapshot().unwrap().calls {
            assert!(c.candidates.is_empty(), "{c:?}");
        }
    }
    #[test]
    fn code_index_invalidates_older_payloads_and_project_scope_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
        let mut main = doc(
            "main",
            "/p/main.py",
            "from a import go\ndef main():\n    go()\n",
        );
        main.wing = Some("one".into());
        let mut lib = doc("lib", "/p/a.py", "def go():\n    pass\n");
        lib.wing = Some("two".into());
        store.upsert_document(&main).unwrap();
        store.upsert_document(&lib).unwrap();
        store.index_code().unwrap();
        assert!(store.code_snapshot().unwrap().calls[0]
            .candidates
            .is_empty());
        // Cache format changes force a parse even when content is unchanged.
        {
            let conn = store.lock().unwrap();
            let payload: String = conn
                .query_row(
                    "SELECT payload FROM code_index_documents WHERE document_id='main'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let mut payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
            payload["version"] = serde_json::json!(1);
            conn.execute(
                "UPDATE code_index_documents SET payload=? WHERE document_id='main'",
                [payload.to_string()],
            )
            .unwrap();
        }
        assert_eq!(store.code_snapshot().unwrap().stale_documents, vec!["main"]);
        assert_eq!(store.index_code().unwrap().indexed, 1);
        let mut lib = store.get_document("lib").unwrap().unwrap();
        lib.wing = Some("one".into());
        store.upsert_document(&lib).unwrap();
        store.index_code().unwrap();
        assert_eq!(
            store.code_snapshot().unwrap().calls[0].resolution,
            "resolved_static"
        );
    }
    #[test]
    fn code_index_reports_parse_errors_and_has_source_evidence() {
        let parsed = parse(
            "d",
            "a.rs",
            "// Привет\nfn alpha() { missing(); }\nfn broken(",
            4,
        )
        .unwrap();
        assert!(parsed.parse_errors);
        let call = parsed.references.iter().find(|r| r.is_call).unwrap();
        assert_eq!(call.line, 2);
        assert!(!call.content_hash.is_empty());
        let symbol = parsed.symbols.iter().find(|s| s.name == "alpha").unwrap();
        assert_eq!(symbol.start_line, 2);
        assert_eq!(symbol.signature, "fn alpha()");
    }
}
