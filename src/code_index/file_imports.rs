//! File-level static import candidates, independent of exported symbol lookup.
use super::*;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Spec {
    module: String,
    kind: String,
    line: usize,
    start_byte: usize,
    initializer_only: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct FileImportEdge {
    pub document_id: String,
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub content_hash: String,
    pub module: String,
    pub kind: String,
    pub line: usize,
    pub start_byte: usize,
    pub candidates: Vec<String>,
    pub resolution: &'static str,
    pub resolution_basis: &'static str,
    pub runtime_import_proven: bool,
}
fn push(
    out: &mut IndexedDocument,
    node: Node<'_>,
    module: &str,
    kind: &str,
    initializer_only: bool,
) {
    out.file_imports.push(Spec {
        module: module.into(),
        kind: kind.into(),
        line: node.start_position().row + 1,
        start_byte: node.start_byte(),
        initializer_only,
    });
}
fn python(out: &mut IndexedDocument, node: Node<'_>, module: &str, kind: &str) {
    push(out, node, module, kind, false);
    let dots = module.chars().take_while(|c| *c == '.').count();
    let parts: Vec<_> = module[dots..]
        .split('.')
        .filter(|p| !p.is_empty())
        .collect();
    for count in 1..parts.len() {
        push(
            out,
            node,
            &format!("{}{}", ".".repeat(dots), parts[..count].join(".")),
            "python_package_initializer_candidate",
            true,
        );
    }
}
pub(super) fn collect(node: Node<'_>, source: &str, out: &mut IndexedDocument) {
    if out.path.ends_with(".py") {
        match node.kind() {
            "import_statement" => {
                let mut cursor = node.walk();
                for name in node.children_by_field_name("name", &mut cursor) {
                    let name = name.child_by_field_name("name").unwrap_or(name);
                    python(out, node, text(name, source), "python_import");
                }
            }
            "import_from_statement" => {
                if let Some(module) = node.child_by_field_name("module_name") {
                    let module = text(module, source);
                    python(out, node, module, "python_from_import");
                    let mut cursor = node.walk();
                    for name in node.children_by_field_name("name", &mut cursor) {
                        let name = name.child_by_field_name("name").unwrap_or(name);
                        let separator = if module.ends_with('.') { "" } else { "." };
                        push(
                            out,
                            node,
                            &format!("{module}{separator}{}", text(name, source)),
                            "python_from_submodule_candidate",
                            false,
                        );
                    }
                }
            }
            _ => {}
        }
    } else if matches!(
        Path::new(&out.path).extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs")
    ) && matches!(node.kind(), "import_statement" | "export_statement")
    {
        if let Some(module) = node.child_by_field_name("source") {
            let raw = text(module, source);
            if raw.len() >= 2 {
                push(
                    out,
                    node,
                    &raw[1..raw.len() - 1],
                    if node.kind() == "export_statement" {
                        "esm_reexport"
                    } else {
                        "esm_import"
                    },
                    false,
                );
            }
        }
    }
}
pub(super) fn ruby_import(node: Node<'_>, module: &str, name: &str, out: &mut IndexedDocument) {
    push(
        out,
        node,
        module,
        if name == "require_relative" {
            "ruby_require_relative_candidate"
        } else {
            "ruby_load_path_unresolved"
        },
        false,
    );
}
pub(super) fn ruby_paths(path: &str, module: &str) -> Vec<String> {
    if module.is_empty() || module.starts_with('/') || module.contains(['\\', '?', '#', '%', '\0'])
    {
        return vec![];
    }
    let Some(parent) = Path::new(path).parent() else {
        return vec![];
    };
    let base = parent.join(module);
    if base.extension().is_some() {
        vec![resolve::normalized(&base)]
    } else {
        vec![resolve::normalized(&base.with_extension("rb"))]
    }
}
pub(super) fn perl_import(node: Node<'_>, module: &str, kind: &str, out: &mut IndexedDocument) {
    push(out, node, module, kind, false);
}
pub(super) fn powershell_import(node: Node<'_>, path: &str, kind: &str, out: &mut IndexedDocument) {
    push(out, node, path, kind, false);
}
pub(super) fn edges(documents: &HashMap<String, IndexedDocument>) -> Vec<FileImportEdge> {
    let mut out = Vec::new();
    for (id, doc) in documents.iter().filter(|(_, d)| !d.parse_errors) {
        for spec in &doc.file_imports {
            let supported = if doc.powershell.is_some() {
                spec.module.starts_with('/')
            } else if doc.perl.is_some() {
                spec.kind.starts_with("perl_")
            } else if doc.ruby.is_some() {
                spec.kind == "ruby_require_relative_candidate"
                    && !ruby_paths(&doc.path, &spec.module).is_empty()
            } else if doc.path.ends_with(".py") {
                !spec.module.is_empty()
                    && spec
                        .module
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            } else {
                (spec.module.starts_with("./") || spec.module.starts_with("../"))
                    && !spec.module.contains(['\\', '?', '#', '%'])
            };
            let paths = if supported && doc.perl.is_none() {
                let mut paths = if doc.powershell.is_some() {
                    vec![spec.module.clone()]
                } else if doc.ruby.is_some() {
                    ruby_paths(&doc.path, &spec.module)
                } else {
                    resolve::module_paths(&doc.path, &spec.module)
                };
                if doc.path.ends_with(".py") && spec.module.chars().all(|c| c == '.') {
                    paths.retain(|p| p.ends_with("/__init__.py"));
                }
                if spec.initializer_only {
                    paths.retain(|p| p.ends_with("/__init__.py"));
                }
                paths
            } else {
                vec![]
            };
            let mut candidates: Vec<_> = documents
                .iter()
                .filter(|(target, d)| {
                    *target != id
                        && !d.parse_errors
                        && d.project == doc.project
                        && if doc.perl.is_some() {
                            super::perl::package_matches(d, &spec.module)
                        } else {
                            paths.contains(&d.path)
                        }
                })
                .map(|(id, _)| id.clone())
                .collect();
            candidates.sort();
            candidates.dedup();
            out.push(FileImportEdge {
                document_id: id.clone(),
                path: doc.path.clone(),
                project: doc.project.clone(),
                revision: doc.revision,
                content_hash: doc.hash.clone(),
                module: spec.module.clone(),
                kind: spec.kind.clone(),
                line: spec.line,
                start_byte: spec.start_byte,
                resolution: match candidates.len() {
                    0 => "unresolved",
                    1 => "resolved_static_candidate",
                    _ => "ambiguous",
                },
                candidates,
                resolution_basis: if doc.powershell.is_some() {
                    "captured_literal_source_relative_or_PSScriptRoot_path;runspace_load_unverified"
                } else if doc.perl.is_some() {
                    "captured_same_project_package_declarations;INC_and_execution_unverified"
                } else if supported {
                    "captured_same_project_module_paths"
                } else {
                    "unsupported_loader_or_specifier"
                },
                runtime_import_proven: false,
            });
        }
    }
    out.sort_by(|a, b| {
        (&a.document_id, a.start_byte, &a.kind, &a.module).cmp(&(
            &b.document_id,
            b.start_byte,
            &b.kind,
            &b.module,
        ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn documents(files: &[(&str, &str, &str)]) -> HashMap<String, IndexedDocument> {
        files
            .iter()
            .enumerate()
            .map(|(i, (path, project, body))| {
                let mut d = parse(&i.to_string(), path, body, 1).unwrap();
                assert!(!d.parse_errors, "{path}");
                d.project = Some((*project).into());
                (i.to_string(), d)
            })
            .collect()
    }
    #[test]
    fn missing_export_and_symbol_free_side_effect_imports_keep_file_edges() {
        let docs = documents(&[
            ("/p/api.py", "p", "VALUE = 1\n"),
            ("/p/client.py", "p", "from api import absent\n"),
            ("/p/effect.js", "p", "console.log('ready');"),
            (
                "/p/entry.ts",
                "p",
                "import './effect.js'; export {missing} from './effect.js';",
            ),
        ]);
        let edges = edges(&docs);
        assert!(edges
            .iter()
            .any(|e| e.document_id == "1" && e.candidates == ["0"]));
        assert_eq!(
            edges
                .iter()
                .filter(|e| e.document_id == "3" && e.candidates == ["2"])
                .count(),
            2
        );
        assert!(resolve::references(
            &docs
                .values()
                .flat_map(|d| d.symbols.clone())
                .collect::<Vec<_>>(),
            &docs
        )
        .iter()
        .all(|e| !e.reference.is_call || e.candidates.is_empty()));
    }
    #[test]
    fn python_packages_relative_submodules_and_cross_project_guards() {
        let docs = documents(&[
            ("/p/pkg/__init__.py", "p", ""),
            ("/p/pkg/child.py", "p", ""),
            (
                "/p/pkg/use.py",
                "p",
                "from . import child\nimport pkg.child as alias\n",
            ),
            ("/p/pkg/child.py", "other", ""),
        ]);
        let edges = edges(&docs);
        assert!(edges
            .iter()
            .any(|e| e.kind == "python_from_submodule_candidate" && e.candidates == ["1"]));
        assert!(edges
            .iter()
            .any(|e| e.kind == "python_package_initializer_candidate" && e.candidates == ["0"]));
        assert!(edges.iter().all(|e| !e.candidates.contains(&"3".into())));
    }
    #[test]
    fn ambiguity_and_unsupported_loaders_remain_explicit() {
        let mut docs = documents(&[
            ("/p/m.ts", "p", ""),
            ("/p/m.js", "p", ""),
            (
                "/p/use.ts",
                "p",
                "import './m'; import 'package'; import './m?raw'; const x = import('./m');",
            ),
        ]);
        let got = edges(&docs);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].resolution, "ambiguous");
        assert_eq!(got[0].candidates, ["0", "1"]);
        assert!(got[1..]
            .iter()
            .all(|e| e.candidates.is_empty()
                && e.resolution_basis == "unsupported_loader_or_specifier"));
        docs.get_mut("0").unwrap().parse_errors = true;
        assert_eq!(edges(&docs)[0].candidates, ["1"]);
    }
    #[test]
    fn file_imports_survive_missing_symbols_and_refresh_with_frozen_history() {
        use crate::models::Document;
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("imports.duckdb")).unwrap();
        let doc = |id: &str, path: &str, body: &str| Document {
            id: id.into(),
            uri: format!("file://{path}"),
            source_file: Some(path.into()),
            title: path.into(),
            content: body.into(),
            ..Default::default()
        };
        store
            .upsert_document(&doc("api", "/p/api.py", "VALUE = 1\n"))
            .unwrap();
        let mut client = doc("client", "/p/client.py", "from api import absent\n");
        store.upsert_document(&client).unwrap();
        store.index_code().unwrap();
        let q = CodeQuery {
            impact_file: Some("/p/api.py".into()),
            ..Default::default()
        };
        let snapshot = store.code_snapshot().unwrap();
        let result = snapshot.query("impact", &q);
        assert_eq!(result["items"][0]["path"], "/p/client.py", "{result}");
        assert_eq!(result["items"][0]["via_kind"], "file_import_candidate");
        assert!(snapshot.query(
            "impact",
            &CodeQuery {
                impact_relations: Some("calls".into()),
                impact_file: q.impact_file.clone(),
                ..Default::default()
            }
        )["items"]
            .as_array()
            .unwrap()
            .is_empty());
        let dead = store.code_dead_files(&CodeQuery::default()).unwrap();
        assert!(
            !dead["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["file"]["path"] == "/p/api.py"),
            "{dead}"
        );
        client.content = "x = 1\n".into();
        store.upsert_document(&client).unwrap();
        assert!(store.code_snapshot().unwrap().query("impact", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        store.index_code().unwrap();
        assert!(store.code_snapshot().unwrap().query("impact", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        let diffq = history::DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        };
        let diff = store.code_index_diff(&diffq).unwrap();
        assert_eq!(diff["file_import_relations_available"], true);
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == "file_import_candidate"),
            "{diff}"
        );
        client.content = "from api import absent\n".into();
        store.upsert_document(&client).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&diffq).unwrap(), diff);
        let conn = store.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph.as_object_mut().unwrap().remove("file_imports");
        conn.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(conn);
        let legacy = store.code_index_diff(&diffq).unwrap();
        assert_eq!(legacy["file_import_relations_available"], false);
        assert!(!legacy["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "file_import_candidate"));
    }
}
