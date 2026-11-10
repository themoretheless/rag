
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
        .documents()
        .upsert_document(&doc(
            "a",
            "a.rs",
            "fn alpha() { beta(); external(); } fn beta() {}",
        ))
        .unwrap();
    store
        .documents()
        .upsert_document(&doc("b", "b.rs", "fn beta() {}"))
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .unindexed_documents
            .len(),
        2
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .indexed,
        2
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .unchanged,
        2
    );
    let snap = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
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
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .symbols
            .len(),
        3
    );
    // A source mutation cannot silently return old indexed facts.
    let mut a = store.documents().get_document("a").unwrap().unwrap();
    a.content = "fn replacement() {}".into();
    store.documents().upsert_document(&a).unwrap();
    let snap = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    assert_eq!(snap.stale_documents, vec!["a"]);
    assert!(snap.calls.is_empty());
    store.documents().delete_document("b").unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .symbols[0]
            .name,
        "replacement"
    );
    let mut a = store.documents().get_document("a").unwrap().unwrap();
    a.status = "archived".into();
    store.documents().upsert_document(&a).unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .symbols
        .is_empty());
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
        .documents()
        .upsert_document(&doc(
            "a",
            "a.rs",
            "fn a() { b(); } fn b() { c(); } fn c() { a(); }",
        ))
        .unwrap();
    store
        .documents()
        .upsert_document(&doc("other", "other.rs", "fn c() {}"))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let snapshot = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    let q = CodeQuery {
        symbol: Some("c".into()),
        path: Some("a.rs".into()),
        ..Default::default()
    };
    let body = crate::code_index::CodeIndex::new(&store)
        .code_body(&q)
        .unwrap();
    assert_eq!(body["code"], "fn c() { a(); }");
    let mut q = CodeQuery {
        symbol: Some("c".into()),
        ..Default::default()
    };
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_body(&q)
            .unwrap()["ambiguous"],
        true
    );
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_body(&CodeQuery::default())
        .is_err());
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
            store.documents().upsert_document(&doc("main",main_path,main)).unwrap();
            store.documents().upsert_document(&doc("lib",lib_path,library)).unwrap();
            store.documents().upsert_document(&doc("decoy","/other/decoy.rs","fn go() {} struct Worker; impl Worker { fn work() {} }")).unwrap();
            crate::code_index::CodeIndex::new(&store).index_code().unwrap();let snapshot=crate::code_index::CodeIndex::new(&store).code_snapshot().unwrap();
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
        .documents()
        .upsert_document(&doc(
            "main",
            "/p/main.ts",
            "import {go} from './a'; function main(go, obj) { go(); obj.work(); }",
        ))
        .unwrap();
    store
        .documents()
        .upsert_document(&doc(
            "lib",
            "/p/a.ts",
            "export function go() {} export class Worker { work() {} }",
        ))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    for c in crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .calls
    {
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
            store.documents().upsert_document(&doc("main",main_path,main)).unwrap();store.documents().upsert_document(&doc("lib",lib_path,library)).unwrap();
            crate::code_index::CodeIndex::new(&store).index_code().unwrap(); let snap=crate::code_index::CodeIndex::new(&store).code_snapshot().unwrap();
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
        .documents()
        .upsert_document(&doc(
            "root",
            "/p/src/lib.rs",
            "pub mod inline { pub fn run() {} } pub fn root() {}",
        ))
        .unwrap();
    store
        .documents()
        .upsert_document(&doc(
            "caller",
            "/p/src/child.rs",
            "use crate::inline::run as alias; use super::root; fn caller() { alias(); root(); }",
        ))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    for c in crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .calls
    {
        assert_eq!(c.resolution, "resolved_static", "{c:?}");
        assert!(c.candidates[0].starts_with("root:"));
    }
}
#[test]
fn code_index_external_imports_and_bare_methods_do_not_gain_false_edges() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
    store.documents().upsert_document(&doc("main","/p/main.ts","import { go } from 'external'; import {hidden} from './a'; class Local { run() { work(); go(); hidden(); this.inner.work(); } work() {} } function shadowed() { hidden(); const hidden=()=>{}; } function nested(w: Local) { w.inner.work(); }" )).unwrap();
    store
        .documents()
        .upsert_document(&doc(
            "lib",
            "/p/a.ts",
            "export function go() {} function hidden() {}",
        ))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    for c in crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .calls
    {
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
    store.documents().upsert_document(&main).unwrap();
    store.documents().upsert_document(&lib).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .calls[0]
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
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .stale_documents,
        vec!["main"]
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .indexed,
        1
    );
    let mut lib = store.documents().get_document("lib").unwrap().unwrap();
    lib.wing = Some("one".into());
    store.documents().upsert_document(&lib).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .calls[0]
            .resolution,
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
