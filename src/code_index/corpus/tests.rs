
use super::*;
use crate::models::Document;
fn put(store: &Store, id: &str, path: &str, content: &str, project: &str) {
    store
        .documents()
        .upsert_document(&Document {
            id: id.into(),
            uri: format!("file://{path}"),
            source_file: Some(path.into()),
            content: content.into(),
            wing: Some(project.into()),
            ..Default::default()
        })
        .unwrap();
}
fn search(queries: &[&str]) -> MultiSearch {
    MultiSearch {
        queries: queries.iter().map(|q| q.to_string()).collect(),
        project: None,
        path_prefix: None,
        limit: None,
        match_mode: None,
        regex: false,
        case_insensitive: false,
        literal_mentions: None,
    }
}
#[test]
fn captured_byte_copy_groups_omit_legacy_and_invalidate_changed_text_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("bytes.duckdb")).unwrap();
    for (id, raw) in [
        ("a", b"hello".as_slice()),
        ("b", b"hello".as_slice()),
        ("bom", b"\xef\xbb\xbfhello".as_slice()),
    ] {
        let path = dir.path().join(format!("{id}.txt"));
        std::fs::write(&path, raw).unwrap();
        let extracted = crate::file_ingest::extract_file(&path).unwrap();
        store
            .documents()
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file://{}", path.display()),
                source_file: Some(path.display().to_string()),
                content: extracted.text,
                metadata_json: serde_json::to_string(&extracted.metadata).unwrap(),
                wing: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
    }
    put(&store, "legacy", "/p/legacy.txt", "hello", "p");
    let q = DuplicateQuery {
        project: Some("p".into()),
        grouping: Some(DuplicateGrouping::SourceByteCopies),
        literal_mentions: Some(LiteralMentionOptions {
            enabled: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(report["groups_total"], 1, "{report}");
    assert_eq!(report["groups"][0]["kind"], "exact_captured_source_bytes");
    assert_eq!(report["groups"][0]["members_total"], 2);
    assert_eq!(
        report["source_byte_identity"]["capture_status_counts"]["legacy_or_missing_capture"],
        1
    );
    let mut doc = store.documents().get_document("b").unwrap().unwrap();
    doc.content = "changed".into();
    store.documents().upsert_document(&doc).unwrap();
    let changed = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(changed["groups_total"], 0);
    assert_eq!(
        changed["source_byte_identity"]["capture_status_counts"]["source_text_mismatch"],
        1
    );
}
#[test]
fn duplicate_family_inventory_keeps_independent_exact_and_dissimilar_groups() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("families.duckdb")).unwrap();
    for (id, path, body) in [
        ("a", "/p/Files.txt", "PRIVATE_ALPHA"),
        ("b", "/p/Files2.txt", "PRIVATE_ALPHA"),
        ("c", "/p/Files3.txt", "PRIVATE_OMEGA"),
        ("clone", "/p/unrelated.txt", "PRIVATE_ALPHA"),
        ("d", "/p/Other.txt", "PRIVATE_DELTA"),
        ("e", "/p/Other2.txt", "PRIVATE_EPSILON"),
        ("nested", "/p/nested/Other3.txt", "PRIVATE_NESTED"),
        ("case", "/p/other3.txt", "PRIVATE_CASE"),
        ("x", "/p/Exact.txt", "PRIVATE_EXACT"),
        ("y", "/p/Exact2.txt", "PRIVATE_EXACT"),
    ] {
        put(&store, id, path, body, "p");
    }
    put(&store, "foreign", "/p/Other4.txt", "PRIVATE_FOREIGN", "q");
    let query = DuplicateQuery {
        grouping: Some(DuplicateGrouping::ExactAndNameFamilies),
        project: Some("p".into()),
        literal_mentions: Some(LiteralMentionOptions {
            enabled: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&query)
        .unwrap();
    assert_eq!(report["groups_total"], 4, "{report}");
    assert_eq!(report["comparisons"], 0);
    let groups = report["groups"].as_array().unwrap();
    assert!(groups[..2].iter().all(|g| g["kind"] == "exact_stored_text"));
    let family = groups
        .iter()
        .find(|g| g["family_key"] == "p/Files.txt")
        .unwrap();
    assert_eq!(family["kind"], "family");
    assert_eq!(family["members_total"], 3);
    assert_eq!(family["family_classification"]["complete"], true);
    let different = groups
        .iter()
        .find(|g| g["family_key"] == "p/Other.txt")
        .unwrap();
    assert_eq!(different["kind"], "same_name");
    assert_eq!(different["members_total"], 2);
    assert!(!report.to_string().contains("PRIVATE_"));
    assert_eq!(
        report,
        crate::code_index::CodeIndex::new(&store)
            .duplicates(&query)
            .unwrap()
    );
    let scoped = crate::code_index::CodeIndex::new(&store)
        .duplicates(&DuplicateQuery {
            path_prefix: Some("/p/nested".into()),
            ..query
        })
        .unwrap();
    assert_eq!(scoped["groups_total"], 0);
}
#[test]
fn duplicate_family_limits_do_not_manufacture_same_name_classification() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("limits.duckdb")).unwrap();
    put(&store, "a", "/p/Files.txt", "alpha", "p");
    put(&store, "b", "/p/Files2.txt", "beta", "p");
    let mut q = DuplicateQuery {
        grouping: Some(DuplicateGrouping::ExactAndNameFamilies),
        project: Some("p".into()),
        literal_mentions: Some(LiteralMentionOptions {
            enabled: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    q.max_members = Some(1);
    let r = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(r["groups"][0]["kind"], "not_checked");
    assert_eq!(r["primary_comparison"]["pairs_requested"], 0);
    q.max_members = None;
    q.primary_comparison = Some(PrimaryComparisonOptions {
        enabled: Some(false),
        ..Default::default()
    });
    let r = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(r["groups"][0]["kind"], "not_checked");
    q.primary_comparison = Some(PrimaryComparisonOptions {
        max_file_bytes: Some(1),
        ..Default::default()
    });
    let r = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(r["groups"][0]["kind"], "not_checked");
    q.primary_comparison = None;
    let r = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(r["groups"][0]["kind"], "same_name");
    q.grouping = None;
    let r = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(r["groups_total"], 0);
}
#[test]
fn duplicate_primary_comparison_uses_returned_window_and_current_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("comparison.duckdb")).unwrap();
    let shared = (0..10)
        .map(|i| format!("shared private content line {i}\n"))
        .collect::<String>();
    put(
        &store,
        "a",
        "/p/sample.txt",
        &format!("{shared}PRIVATE_ALPHA\n"),
        "p",
    );
    put(
        &store,
        "b",
        "/p/sample2.txt",
        &format!("{shared}PRIVATE_BETA\n"),
        "p",
    );
    let mut query = DuplicateQuery {
        project: Some("p".into()),
        min_similarity: Some(0.5),
        literal_mentions: Some(LiteralMentionOptions {
            enabled: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&query)
        .unwrap();
    assert_eq!(report["groups_total"], 1, "{report}");
    assert_eq!(report["primary_comparison"]["pairs_checked"], 1);
    let group = &report["groups"][0];
    let member = group["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["document"]["document_id"] != group["primary_candidate"])
        .unwrap();
    assert_eq!(member["similarity_to_primary"], 0.833);
    assert_eq!(member["lines_only_here"], 1);
    assert_eq!(member["lines_only_in_primary"], 1);
    assert!(!report.to_string().contains("PRIVATE_"));
    query.max_members = Some(1);
    let limited = crate::code_index::CodeIndex::new(&store)
        .duplicates(&query)
        .unwrap();
    assert_eq!(limited["primary_comparison"]["pairs_requested"], 0);
    assert_eq!(
        limited["primary_comparison"]["decoded_input_bytes_charged"],
        0
    );
}
#[test]
fn multi_search_exact_conjunction_regex_scope_utf8_and_no_chunks_needed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.duckdb")).unwrap();
    put(
        &store,
        "a",
        "/p/a.txt",
        "Привет 🦀\nalpha|beta\nSECOND",
        "p",
    );
    put(&store, "b", "/p/ab/b.txt", "alpha|beta", "q");
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&search(&["alpha|beta", "SECOND"]))
        .unwrap();
    assert_eq!(result["matching_documents_total"], 1);
    assert_eq!(result["items"][0]["matches"][0]["hits"][0]["line"], 2);
    let mut q = search(&["alpha|beta", "SECOND"]);
    q.match_mode = Some("any".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&q)
            .unwrap()["matching_documents_total"],
        2
    );
    q.project = Some("p".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&q)
            .unwrap()["documents_scanned"],
        1
    );
    q = search(&["alpha\\|b.ta", "second"]);
    q.regex = true;
    q.case_insensitive = true;
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&q)
            .unwrap()["matching_documents_total"],
        1
    );
    q = search(&["🦀\nalpha"]);
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&q)
            .unwrap()["items"][0]["matches"][0]["hits"][0]["column"],
        8
    );
    q.path_prefix = Some("/p/a".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&q)
            .unwrap()["documents_scanned"],
        0
    );
    q.regex = true;
    q.queries = vec!["a*".into()];
    assert!(crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .is_err());
    q.queries = vec!["(".into()];
    assert!(crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .is_err());
}
#[test]
fn independent_query_windows_and_families_preserve_scope_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("multi.duckdb")).unwrap();
    put(&store, "z", "/p/Files.php", "alpha alpha alpha alpha", "p");
    put(&store, "y", "/p/Files2.php", "beta", "p");
    put(&store, "x", "/q/Files3.php", "alpha", "q");
    let mut q = search(&["alpha", "beta", "absent"]);
    q.limit = Some(1);
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .unwrap();
    assert_eq!(result["matching_documents_total"], 0);
    assert_eq!(result["union_matching_documents_total"], 3);
    assert_eq!(result["per_query"][0]["matching_documents_total"], 2);
    assert_eq!(result["per_query"][0]["truncated"], true);
    assert_eq!(
        result["per_query"][0]["items"][0]["hits"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(result["per_query"][0]["items"][0]["more_occurrences"], true);
    assert_eq!(
        result["per_query"][1]["items"][0]["document"]["path"],
        "/p/Files2.php"
    );
    assert_eq!(result["per_query"][2]["items"], serde_json::json!([]));
    assert_eq!(result["families"][0]["duplicate_content_proven"], false);
    assert_eq!(result["families"][0]["truncated"], true);
    q.limit = Some(10);
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .unwrap();
    assert_eq!(result["families_retained_total"], 2);
    assert_eq!(
        result["families"][0]["members"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        result,
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&q)
            .unwrap()
    );
    q.project = Some("q".into());
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .unwrap();
    assert_eq!(result["union_matching_documents_total"], 1);
    assert_eq!(result["per_query"][1]["matching_documents_total"], 0);
    assert_eq!(result["families"][0]["project"], "q");
    assert_eq!(
        result["per_query"][0]["items"][0]["source_encoding"],
        serde_json::Value::Null
    );
}

#[test]
fn source_encoding_labels_require_matching_decoded_source_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("encoding.duckdb")).unwrap();
    put(&store, "doc", "/p/code.py", "needle", "p");
    let mut doc = store.documents().get_document("doc").unwrap().unwrap();
    doc.metadata_json = serde_json::json!({"source_decoding_version":1,"source_encoding":"windows-1251","source_encoding_basis":"heuristic_cyrillic_share","source_text_hash":content_hash("needle"),"source_bytes_hash":"raw"}).to_string();
    store.documents().upsert_document(&doc).unwrap();
    let query = search(&["needle"]);
    let found = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(
        found["per_query"][0]["items"][0]["source_encoding"],
        "windows-1251"
    );
    doc.content = "needle changed".into();
    store.documents().upsert_document(&doc).unwrap();
    let found = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(
        found["per_query"][0]["items"][0]["source_encoding"],
        serde_json::Value::Null
    );
    assert_eq!(
        found["per_query"][0]["items"][0]["source_bytes_hash"],
        serde_json::Value::Null
    );
}

#[test]
fn search_family_roles_use_bound_values_and_omit_stale_callers_and_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("roles.duckdb")).unwrap();
    put(
        &store,
        "api",
        "/p/api.py",
        "def work():\n    return 'needle'\n",
        "p",
    );
    put(
        &store,
        "copy",
        "/p/api2.py",
        "def other():\n    return 'needle'\n",
        "p",
    );
    put(
        &store,
        "caller",
        "/p/use.py",
        "from api import work\nregistered = work\n",
        "p",
    );
    put(
        &store,
        "unreferenced",
        "/p/aaa.py",
        "def unused():\n    return 'needle'\n",
        "p",
    );
    put(
        &store,
        "caller",
        "/p/use.py",
        &format!(
            "from api import work\n{}",
            "registered = work\n".repeat(105)
        ),
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = search(&["needle"]);
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .unwrap();
    let members = &result["families"][0]["members"];
    assert_eq!(members[0]["document"]["path"], "/p/api.py");
    assert_eq!(members[0]["role"], "referenced_candidate");
    assert!(members[0]["role_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "incoming_reference_candidate"
            && e["path"] == "/p/use.py"
            && e["content_hash"].is_string()));
    assert_eq!(members[0]["role_evidence"].as_array().unwrap().len(), 100);
    assert_eq!(members[0]["role_evidence_truncated"], true);
    assert!(members[0]["role_evidence_total"].as_u64().unwrap() >= 105);
    put(&store, "caller", "/p/use.py", "registered = None\n", "p");
    let stale = crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .unwrap();
    assert!(!stale["family_role_analysis"]["stale_documents"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(stale["families"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["members"].as_array().unwrap().iter())
        .all(|m| m["usage_score"].as_u64().unwrap() < 3));
    put(
        &store,
        "caller",
        "/p/use.py",
        "from api import work\nregistered = work\n",
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    put(
        &store,
        "api",
        "/p/api.py",
        "def changed():\n    return 'needle'\n",
        "p",
    );
    let stale = crate::code_index::CodeIndex::new(&store)
        .search_multi(&q)
        .unwrap();
    assert!(stale["families"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["members"].as_array().unwrap().iter())
        .all(|m| m["usage_score"].as_u64().unwrap() < 3));
}

#[test]
fn exact_and_numbered_copies_have_static_liveness_evidence_and_no_delete_claim() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("dups.duckdb")).unwrap();
    put(
        &store,
        "base",
        "/p/copies/files.ts",
        "export function work() { return 1; }",
        "p",
    );
    put(
        &store,
        "active",
        "/p/copies/files7.ts",
        "export function work() { return 1; }",
        "p",
    );
    put(
        &store,
        "caller",
        "/p/main.ts",
        "import {work} from './copies/files7'; function main() { work(); }",
        "p",
    );
    put(
        &store,
        "other",
        "/q/files.ts",
        "export function work() { return 1; }",
        "q",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let result = crate::code_index::CodeIndex::new(&store)
        .duplicates(&DuplicateQuery {
            project: Some("p".into()),
            path_prefix: Some("/p/copies".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result["groups_total"], 1);
    assert_eq!(result["groups"][0]["primary_candidate"], "active");
    assert_eq!(result["groups"][0]["deletion_safe"], false);
    assert!(result["groups"][0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["document"]["document_id"] == "active" && m["role"] == "referenced_candidate"));
    assert!(!result.to_string().contains("return 1"));
}
#[test]
fn approximate_families_have_threshold_evidence_and_explicit_work_cap() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("dups.duckdb")).unwrap();
    for i in 0..4 {
        put(
            &store,
            &i.to_string(),
            &format!("/p/files{i}.txt"),
            &format!("one two three four five six seven eight nine ten number{i}"),
            "p",
        );
    }
    let q = DuplicateQuery {
        min_similarity: Some(0.5),
        max_comparisons: Some(1),
        ..Default::default()
    };
    let result = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(result["comparison_limit_reached"], true);
    assert_eq!(result["comparisons"], 1);
    assert_eq!(result["groups_total"], 1);
    assert_eq!(
        result["groups"][0]["similarity_evidence"][0]["kind"],
        "name_family_and_token_shingles"
    );
}
#[test]
fn duplicate_roles_include_imports_and_resources_from_outside_target_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("roles.duckdb")).unwrap();
    for (id, path, body) in [
        ("css1", "/p/copies/theme.css", "body { color:red; }"),
        ("css2", "/p/copies/theme2.css", "body { color:red; }"),
        ("js1", "/p/copies/mod.js", "const value=1;"),
        ("js2", "/p/copies/mod2.js", "const value=1;"),
        (
            "html",
            "/p/index.html",
            "<link rel='stylesheet' href='copies/theme2.css'>",
        ),
        (
            "import",
            "/p/use.js",
            "import {missing} from './copies/mod2.js';",
        ),
    ] {
        put(&store, id, path, body, "p");
    }
    put(
        &store,
        "foreign",
        "/q/index.html",
        "<link rel='stylesheet' href='../p/copies/theme.css'>",
        "q",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DuplicateQuery {
        project: Some("p".into()),
        path_prefix: Some("/p/copies".into()),
        ..Default::default()
    };
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    let groups = report["groups"].as_array().unwrap();
    let css = groups
        .iter()
        .find(|g| g["primary_candidate"] == "css2")
        .unwrap_or_else(|| panic!("{report}"));
    assert!(css["members"][0]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "incoming_file_resource_candidate" && e["path"] == "/p/index.html"));
    let js = groups
        .iter()
        .find(|g| g["primary_candidate"] == "js2")
        .unwrap_or_else(|| panic!("{report}"));
    assert!(js["members"][0]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "incoming_file_import_candidate" && e["path"] == "/p/use.js"));
    assert_eq!(report["role_contract_version"], 2);
    assert_eq!(css["primary_is_proven"], false);
    assert_eq!(js["deletion_safe"], false);
}
#[test]
fn duplicate_roles_include_function_values_and_drop_stale_sources() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("values.duckdb")).unwrap();
    for (id, path) in [("base", "/p/copies/api.py"), ("used", "/p/copies/api2.py")] {
        put(
            &store,
            id,
            path,
            "def work():\n return 'PRIVATE_BODY'\n",
            "p",
        );
    }
    put(
        &store,
        "source",
        "/p/use.py",
        "from copies.api2 import work\nregistered=work\n",
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DuplicateQuery {
        project: Some("p".into()),
        path_prefix: Some("/p/copies".into()),
        ..Default::default()
    };
    let before = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(before["groups"][0]["primary_candidate"], "used", "{before}");
    assert!(before["groups"][0]["members"][0]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "incoming_reference_candidate"));
    assert!(!before.to_string().contains("PRIVATE_BODY"));
    put(&store, "source", "/p/use.py", "registered=None\n", "p");
    let after = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    assert_eq!(after["groups"][0]["usage_score"], 0, "{after}");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .duplicates(&q)
            .unwrap()["groups"][0]["usage_score"],
        0
    );
}
#[test]
fn duplicate_resource_target_identity_and_parsed_target_fingerprints_are_required() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("identity.duckdb")).unwrap();
    for (id, path) in [("a", "/p/icon.svg"), ("b", "/p/icon2.svg")] {
        put(&store, id, path, "<svg><path d='M0 0'/></svg>", "p");
    }
    put(
        &store,
        "html",
        "/p/index.html",
        "<img src='icon2.svg'>",
        "p",
    );
    put(&store, "module", "/p/mod.js", "const value=1;", "p");
    put(&store, "import", "/p/use.js", "import './mod.js';", "p");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let files = crate::code_index::CodeIndex::new(&store)
        .corpus_files(Some("p"), None)
        .unwrap();
    let snap = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    let roles = search_roles(&snap, &files);
    assert_eq!(roles.get("b").unwrap().0, 3);
    assert_eq!(roles.get("module").unwrap().0, 3);
    let mut changed = files;
    for file in &mut changed {
        if file.document_id == "b" || file.document_id == "module" {
            file.content_hash = content_hash("changed-between-observations");
        }
    }
    let roles = search_roles(&snap, &changed);
    assert!(!roles.contains_key("b"));
    assert!(!roles.contains_key("module"));
}
#[test]
fn duplicate_evidence_counts_and_bounded_retention_are_order_independent() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("cap.duckdb")).unwrap();
    put(&store, "a", "/p/mod.js", "const value=1;", "p");
    put(&store, "b", "/p/mod2.js", "const value=1;", "p");
    put(
        &store,
        "source",
        "/p/use.js",
        &"import './mod2.js';\n".repeat(105),
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DuplicateQuery {
        project: Some("p".into()),
        max_members: Some(1),
        ..Default::default()
    };
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&q)
        .unwrap();
    let group = &report["groups"][0];
    assert_eq!(group["primary_candidate"], "b");
    assert_eq!(group["members_total"], 2);
    assert_eq!(group["members"][0]["evidence_total"], 105);
    assert_eq!(
        group["members"][0]["evidence"].as_array().unwrap().len(),
        100
    );
    assert_eq!(group["members"][0]["evidence_truncated"], true);
    let files = crate::code_index::CodeIndex::new(&store)
        .corpus_files(Some("p"), None)
        .unwrap();
    let mut snap = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    let before = search_roles(&snap, &files);
    snap.file_imports.reverse();
    assert_eq!(before, search_roles(&snap, &files));
}
#[test]
fn duplicate_primary_remains_in_member_window_and_counts_break_role_ties() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("order.duckdb")).unwrap();
    put(
        &store,
        "long",
        "/p/a/deep/theme.css",
        "body{color:red}",
        "p",
    );
    put(&store, "short", "/p/z/theme2.css", "body{color:red}", "p");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let query = DuplicateQuery {
        project: Some("p".into()),
        max_members: Some(1),
        ..Default::default()
    };
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&query)
        .unwrap();
    assert_eq!(report["groups"][0]["primary_candidate"], "short");
    assert_eq!(
        report["groups"][0]["members"][0]["document"]["document_id"],
        "short"
    );
    put(
        &store,
        "html",
        "/p/index.html",
        "<link href='a/deep/theme.css'><link href='a/deep/theme.css'><link href='z/theme2.css'>",
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let report = crate::code_index::CodeIndex::new(&store)
        .duplicates(&query)
        .unwrap();
    assert_eq!(report["groups"][0]["primary_candidate"], "long", "{report}");
    assert_eq!(report["groups"][0]["members"][0]["evidence_total"], 2);
    assert_eq!(
        report["groups"][0]["members"][0]["document"]["document_id"],
        "long"
    );
}
#[test]
fn search_roles_include_imports_resources_and_sources_outside_target_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("roles.duckdb")).unwrap();
    for (id, path, body) in [
        ("css1", "/p/copies/theme.css", "/*needle*/body{color:red}"),
        ("css2", "/p/copies/theme2.css", "/*needle*/body{color:red}"),
        ("js1", "/p/copies/mod.js", "//needle\nconst value=1;"),
        ("js2", "/p/copies/mod2.js", "//needle\nconst value=1;"),
        (
            "svg1",
            "/p/copies/icon.svg",
            "<svg><text>needle</text></svg>",
        ),
        (
            "svg2",
            "/p/copies/icon2.svg",
            "<svg><text>needle</text></svg>",
        ),
        (
            "html",
            "/p/index.html",
            "<link href='copies/theme2.css'><img src='copies/icon2.svg'>",
        ),
        (
            "import",
            "/p/use.js",
            "import {missing} from './copies/mod2.js';",
        ),
    ] {
        put(&store, id, path, body, "p");
    }
    put(
        &store,
        "foreign",
        "/q/index.html",
        "<link href='../p/copies/theme.css'>",
        "q",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    query.path_prefix = Some("/p/copies".into());
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["documents_scanned"], 6);
    assert_eq!(result["matching_documents_total"], 6);
    assert_eq!(result["families_retained_total"], 3);
    for (family, used, kind) in [
        ("theme.css", "css2", "incoming_file_resource_candidate"),
        ("mod.js", "js2", "incoming_file_import_candidate"),
        ("icon.svg", "svg2", "incoming_file_resource_candidate"),
    ] {
        let group = result["families"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["name"] == family)
            .unwrap();
        let first = &group["members"][0];
        assert_eq!(first["document"]["document_id"], used, "{result}");
        assert_eq!(first["usage_score"], 3);
        assert!(first["role_evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == kind && e["path"].as_str().unwrap().starts_with("/p/")));
    }
    assert_eq!(result["family_role_analysis"]["role_contract_version"], 2);
    assert_eq!(
        result["family_role_analysis"]["markup_references_included"],
        true
    );
    assert_eq!(result["observation_consistency"], "single_read_transaction");
    assert_eq!(
        result,
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()
    );
}
#[test]
fn search_resource_roles_exclude_unindexed_and_stale_sources_and_targets() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("stale.duckdb")).unwrap();
    for (id, path) in [("a", "/p/copies/theme.css"), ("b", "/p/copies/theme2.css")] {
        put(&store, id, path, "/*needle*/body{color:red}", "p");
    }
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    put(
        &store,
        "html",
        "/p/index.html",
        "<link href='copies/theme2.css'>",
        "p",
    );
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    query.path_prefix = Some("/p/copies".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        0
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["members"][0]["document"]["document_id"],
        "b"
    );
    put(
        &store,
        "b",
        "/p/copies/theme2.css",
        "/*needle*/body{color:blue}",
        "p",
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        0
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        3
    );
    put(&store, "html", "/p/index.html", "<main>removed</main>", "p");
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        0
    );
}
#[test]
fn search_scopes_index_reads_and_skips_role_index_for_empty_windows() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("scope.duckdb")).unwrap();
    put(&store, "p", "/p/a.py", "def work(): return 'needle'", "p");
    put(&store, "q", "/q/a.py", "def work(): return 'needle'", "q");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_documents SET payload='not-json' WHERE document_id='q'",
            [],
        )
        .unwrap();
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["matching_documents_total"],
        1
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_documents SET payload='not-json' WHERE document_id='p'",
            [],
        )
        .unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .is_err());
    query.queries = vec!["not-present".into()];
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["matching_documents_total"], 0);
    assert!(result["family_role_analysis"].is_null());
}
#[test]
fn corpus_read_transaction_keeps_roles_consistent_across_a_concurrent_publication() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("snapshot.duckdb");
    let reader = Store::open(&path).unwrap();
    let mut writer = reader.lock().unwrap().try_clone().unwrap();
    put(
        &reader,
        "a",
        "/p/copies/mod.js",
        "//needle\nconst value=1;",
        "p",
    );
    put(
        &reader,
        "b",
        "/p/copies/mod2.js",
        "//needle\nconst value=1;",
        "p",
    );
    put(
        &reader,
        "source",
        "/p/use.js",
        "import './copies/mod2.js';",
        "p",
    );
    crate::code_index::CodeIndex::new(&reader)
        .index_code()
        .unwrap();
    let mut conn = reader.lock().unwrap();
    let transaction = conn.transaction().unwrap();
    let files = corpus_files_locked(&transaction, Some("p"), Some("/p/copies")).unwrap();
    // A cloned connection shares this DuckDB instance, unlike a second open.
    // Publish real parsed facts/history while the first read view stays open.
    let write = writer.transaction().unwrap();
    write
        .execute(
            "UPDATE documents SET content='const removed=1;',revision=revision+1 WHERE id='source'",
            [],
        )
        .unwrap();
    let docs = {
        let mut statement=write.prepare("SELECT id,source_file,content,COALESCE(revision,1),wing,source_file IS NOT NULL FROM documents ORDER BY id").unwrap();
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            })
            .unwrap();
        rows.collect::<std::result::Result<Vec<_>, _>>().unwrap()
    };
    let analyses: Vec<_> = docs
        .iter()
        .map(|(id, path, source, revision, project, _)| {
            let mut parsed = parse(id, path, source, *revision).unwrap();
            parsed.project = project.clone();
            (id.clone(), serde_json::to_string(&parsed).unwrap())
        })
        .collect();
    for (id, payload) in &analyses {
        write
            .execute(
                "UPDATE code_index_documents SET payload=? WHERE document_id=?",
                params![payload, id],
            )
            .unwrap();
    }
    let published = history::publish(&write, &docs, &analyses, false).unwrap();
    assert!(published.1);
    assert_eq!(published.0, 2);
    write.commit().unwrap();
    let old = super::super::snapshot_locked(&transaction, Some("p")).unwrap();
    assert_eq!(old.index_revision, Some(1));
    let roles = search_roles(&old, &files);
    assert_eq!(roles.get("b").unwrap().0, 3);
    transaction.commit().unwrap();
    drop(conn);
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    query.path_prefix = Some("/p/copies".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&reader)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        0
    );
    let duplicate = crate::code_index::CodeIndex::new(&reader)
        .duplicates(&DuplicateQuery {
            project: Some("p".into()),
            path_prefix: Some("/p/copies".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(duplicate["groups"][0]["usage_score"], 0);
    assert_eq!(
        duplicate["role_observation_consistency"],
        "single_read_transaction"
    );
}
#[test]
fn scoped_corpus_body_reads_cross_parameter_batches_without_losing_order() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("batches.duckdb")).unwrap();
    store.lock().unwrap().execute("INSERT INTO documents(id,uri,title,content,source_file,wing,created_at,updated_at) SELECT 'd'||i,'file://d'||i,'doc','needle','/p/copies/f'||i||'.txt','p',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP FROM range(513) t(i)",[]).unwrap();
    put(&store, "quoted'?)", "/p/copies/quoted.txt", "needle", "p");
    put(&store, "outside", "/p/other.txt", "outside", "p");
    let mut connection = store.lock().unwrap();
    let tx = connection.transaction().unwrap();
    let files = corpus_files_locked(&tx, Some("p"), Some("/p/copies")).unwrap();
    assert_eq!(files.len(), 514);
    assert!(
        files
            .windows(2)
            .all(|pair| (&pair[0].path, &pair[0].document_id)
                <= (&pair[1].path, &pair[1].document_id))
    );
    assert!(files.iter().any(|f| f.document_id == "quoted'?)"));
    assert!(files.iter().all(|f| f.content == "needle"));
    tx.commit().unwrap();
}
#[test]
fn literal_path_roles_find_endpoint_constants_outside_target_scope() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("literal.duckdb")).unwrap();
    for (id, path) in [("base", "/p/php/Files.php"), ("used", "/p/php/Files2.php")] {
        put(
            &store,
            id,
            path,
            "<?php function work(){return 'needle';}",
            "p",
        );
    }
    put(
        &store,
        "js",
        "/p/app.js",
        "// café\nconst ENDPOINT='php/Files2.php';",
        "p",
    );
    put(
        &store,
        "foreign",
        "/q/app.js",
        "const ENDPOINT='../p/php/Files.php';",
        "q",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    query.path_prefix = Some("/p/php".into());
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    let member = &result["families"][0]["members"][0];
    assert_eq!(member["document"]["document_id"], "used", "{result}");
    assert_eq!(member["usage_score"], 2);
    let evidence = member["role_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "literal_text_path_mention_candidate")
        .unwrap();
    assert_eq!(evidence["line"], 2);
    assert_eq!(evidence["binding_proven"], false);
    assert!(evidence["byte_start"].as_u64().unwrap() > 8);
    assert_eq!(evidence["needle"], "php/Files2.php");
    let dup = crate::code_index::CodeIndex::new(&store)
        .duplicates(&DuplicateQuery {
            project: Some("p".into()),
            path_prefix: Some("/p/php".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(dup["groups"][0]["primary_candidate"], "used");
    assert_eq!(dup["groups"][0]["deletion_safe"], false);
    assert_eq!(dup["literal_path_analysis"]["evidence_total"], 1);
}
#[test]
fn literal_path_basename_uniqueness_uses_the_whole_project_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("ambiguous.duckdb")).unwrap();
    put(&store, "a", "/p/a/Files.php", "<?php echo 'needle';", "p");
    put(&store, "b", "/p/b/Files.php", "<?php echo 'needle';", "p");
    put(&store, "js", "/p/app.js", "const path='Files.php';", "p");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    query.path_prefix = Some("/p/a".into());
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["families"][0]["usage_score"], 0);
    assert_eq!(
        result["literal_path_analysis"]["ambiguous_basename_targets"],
        1
    );
    put(&store, "js", "/p/app.js", "const path='a/Files.php';", "p");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        2
    );
}
#[test]
fn literal_path_boundaries_reject_substrings_unrelated_paths_and_external_urls() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("boundaries.duckdb")).unwrap();
    put(
        &store,
        "target",
        "/p/copies/Files2.php",
        "<?php echo 'needle';",
        "p",
    );
    put(&store,"source","/p/app.js","const a='Files2.php.bak', b='OtherFiles2.php', c='other/Files2.php', d='https://remote/copies/Files2.php';","p");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["families"][0]["usage_score"], 0, "{result}");
    put(
        &store,
        "source",
        "/p/app.js",
        "const path='./copies/Files2.php';",
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        2
    );
}
#[test]
fn literal_path_scan_reports_budgets_disabled_mode_and_stale_sources() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("budgets.duckdb")).unwrap();
    put(
        &store,
        "target",
        "/p/copies/Files2.php",
        "<?php echo 'needle';",
        "p",
    );
    put(
        &store,
        "source",
        "/p/app.js",
        "const path='copies/Files2.php';",
        "p",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut query = search(&["needle"]);
    query.project = Some("p".into());
    query.literal_mentions = Some(LiteralMentionOptions {
        enabled: Some(false),
        ..Default::default()
    });
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["literal_path_analysis"]["status"], "disabled");
    assert_eq!(result["families"][0]["usage_score"], 0);
    query.literal_mentions = Some(LiteralMentionOptions {
        max_bytes: Some(1),
        ..Default::default()
    });
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["literal_path_analysis"]["complete"], false);
    assert_eq!(result["families"][0]["usage_score"], 0);
    assert_eq!(result["literal_path_analysis"]["absence_proven"], false);
    query.literal_mentions = None;
    put(
        &store,
        "source",
        "/p/app.js",
        "const path='copies/Files2.php'; // changed",
        "p",
    );
    let result = crate::code_index::CodeIndex::new(&store)
        .search_multi(&query)
        .unwrap();
    assert_eq!(result["families"][0]["usage_score"], 0);
    assert!(
        result["literal_path_analysis"]["sources_stale_unindexed_or_unqualified"]
            .as_u64()
            .unwrap()
            > 0
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .search_multi(&query)
            .unwrap()["families"][0]["usage_score"],
        2
    );
}
#[test]
fn literal_path_scan_caps_retention_but_keeps_complete_unique_source_counts() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("retention.duckdb")).unwrap();
    for (id, path) in [("a", "/p/copies/Files.php"), ("b", "/p/copies/Files2.php")] {
        put(&store, id, path, "<?php echo 'needle';", "p");
    }
    for i in 0..25 {
        put(
            &store,
            &format!("s{i}"),
            &format!("/p/ref{i:02}.js"),
            "const path='copies/Files2.php'; // copies/Files2.php",
            "p",
        );
    }
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let query = DuplicateQuery {
        project: Some("p".into()),
        path_prefix: Some("/p/copies".into()),
        ..Default::default()
    };
    let result = crate::code_index::CodeIndex::new(&store)
        .duplicates(&query)
        .unwrap();
    let member = &result["groups"][0]["members"][0];
    assert_eq!(member["document"]["document_id"], "b");
    assert_eq!(member["evidence_total"], 25);
    assert_eq!(member["evidence"].as_array().unwrap().len(), 20);
    assert_eq!(member["evidence_truncated"], true);
    assert_eq!(result["literal_path_analysis"]["evidence_total"], 25);
    assert_eq!(result["literal_path_analysis"]["evidence_retained"], 20);
    assert_eq!(
        result,
        crate::code_index::CodeIndex::new(&store)
            .duplicates(&query)
            .unwrap()
    );
}
