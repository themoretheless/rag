use super::*;
use crate::models::Document;
fn reconstruct(old: &str, new: &str, diff: &serde_json::Value) -> String {
    let a: Vec<_> = old.split_inclusive('\n').collect();
    let b: Vec<_> = new.split_inclusive('\n').collect();
    let mut result = String::new();
    let mut at = 0;
    for h in diff["edit_hunks"]["items"].as_array().unwrap() {
        let start = h["indexed_range"]["start"].as_u64().unwrap() as usize - 1;
        let end = h["indexed_range"]["end_exclusive"].as_u64().unwrap() as usize - 1;
        let live_start = h["live_range"]["start"].as_u64().unwrap() as usize - 1;
        let live_end = h["live_range"]["end_exclusive"].as_u64().unwrap() as usize - 1;
        result.push_str(&a[at..start].concat());
        result.push_str(&b[live_start..live_end].concat());
        at = end;
    }
    result.push_str(&a[at..].concat());
    result
}
#[test]
fn live_diff_separates_edits_and_keeps_half_open_coordinates() {
    let old = "same\nold\nbridge\ntail\n";
    let new = "same\nnew\nbridge\nadded\ntail\n";
    let d = line_diff(old, new);
    assert_eq!(d["edit_hunks"]["total"], 2);
    assert_eq!(d["edit_hunks"]["items"][0]["kind"], "replace");
    assert_eq!(d["edit_hunks"]["items"][1]["kind"], "insert");
    assert_eq!(reconstruct(old, new, &d), new);
    assert!(!d.to_string().contains("bridge"));
}
#[test]
fn live_diff_reconstructs_exhaustive_short_repeated_line_inputs() {
    let texts: Vec<String> = (0..31)
        .map(|n| {
            let len = usize::BITS - (n + 1usize).leading_zeros() - 1;
            (0..len)
                .map(|bit| {
                    if (n + 1) & (1 << bit) == 0 {
                        "a\n"
                    } else {
                        "b\n"
                    }
                })
                .collect()
        })
        .collect();
    for old in &texts {
        for new in &texts {
            let d = line_diff(old, new);
            assert_eq!(d["edit_hunks"]["exact"], true);
            assert_eq!(reconstruct(old, new, &d), *new, "{old:?} -> {new:?}");
            assert_eq!(d, line_diff(old, new));
        }
    }
}
#[test]
fn live_diff_preserves_final_newline_crlf_unicode_and_empty_sides() {
    for (old, new) in [
        ("", "雪\n"),
        ("a\n", ""),
        ("x", "x\n"),
        ("x\r\n", "x\n"),
        ("雪\n", "雪\n"),
    ] {
        let d = line_diff(old, new);
        assert_eq!(reconstruct(old, new, &d), new);
        assert_eq!(d["edit_hunks"]["total"], if old == new { 0 } else { 1 });
    }
}
#[test]
fn live_diff_bounds_cells_and_hunks_without_claiming_full_precision() {
    let old = "old\n".repeat(1100);
    let new = "new\n".repeat(1100);
    let d = line_diff(&old, &new);
    assert_eq!(d["edit_hunks"]["exact"], false);
    assert_eq!(
        d["edit_hunks"]["fallback_reason"],
        "lcs_cell_budget_exceeded"
    );
    assert_eq!(reconstruct(&old, &new, &d), new);
    let old = (0..80)
        .map(|n| format!("old{n}\nkeep{n}\n"))
        .collect::<String>();
    let new = old.replace("old", "new");
    let d = line_diff(&old, &new);
    assert_eq!(d["edit_hunks"]["total"], 80);
    assert_eq!(d["edit_hunks"]["items"].as_array().unwrap().len(), 64);
    assert_eq!(d["edit_hunks"]["truncated"], true);
    assert_eq!(d["edit_hunks"]["exact"], true);
}
fn seed(store: &Store, path: &str, content: &str) {
    store
        .documents()
        .upsert_document(&Document {
            id: "a".into(),
            uri: format!("file://{path}"),
            source_file: Some(path.into()),
            wing: Some("p".into()),
            content: content.into(),
            ..Default::default()
        })
        .unwrap();
    crate::code_index::CodeIndex::new(store)
        .index_code()
        .unwrap();
}
#[test]
fn encoded_live_sources_compare_decoded_text_and_expose_raw_evidence() {
    let text = "// Привет\nfn work() {}\n";
    let cp = encoding_rs::WINDOWS_1251.encode(text).0.into_owned();
    let le = [
        vec![0xff, 0xfe],
        text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
    ]
    .concat();
    let be = [
        vec![0xfe, 0xff],
        text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
    ]
    .concat();
    let bom = [vec![0xef, 0xbb, 0xbf], text.as_bytes().to_vec()].concat();
    for (bytes, encoding) in [
        (cp, "windows-1251"),
        (le, "utf-16le"),
        (be, "utf-16be"),
        (bom, "utf-8-sig"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let path = root.path().join("a.rs");
        std::fs::write(&path, &bytes).unwrap();
        let store = Store::open(&db.path().join("live.duckdb")).unwrap();
        seed(&store, &path.display().to_string(), text);
        let roots = vec![root.path().to_path_buf()];
        let q = SourceQuery {
            document_id: Some("a".into()),
            ..Default::default()
        };
        let fresh = crate::code_index::CodeIndex::new(&store)
            .code_freshness(&q, &roots)
            .unwrap();
        assert_eq!(fresh["all_checked_unchanged"], true, "{encoding}");
        assert_eq!(fresh["items"][0]["observed_source"]["encoding"], encoding);
        let live = crate::code_index::CodeIndex::new(&store)
            .code_live_read(&q, &roots)
            .unwrap();
        assert!(live["content"].as_str().unwrap().contains("Привет"));
        assert_eq!(live["observed_source"]["bytes"], bytes.len());
        assert!(read_live_source(&path.display().to_string(), &roots, bytes.len() - 1).is_err());
        std::fs::write(&path, text).unwrap();
        let recoded = crate::code_index::CodeIndex::new(&store)
            .code_live_read(&q, &roots)
            .unwrap();
        assert_eq!(recoded["status"], "unchanged");
        assert_eq!(recoded["observed_hash"], live["observed_hash"]);
        assert_ne!(
            recoded["observed_source"]["bytes_hash"],
            live["observed_source"]["bytes_hash"]
        );
        std::fs::write(&path, text.replace("work", "changed")).unwrap();
        assert_eq!(
            crate::code_index::CodeIndex::new(&store)
                .code_freshness(&q, &roots)
                .unwrap()["changed"],
            1
        );
    }
}

#[test]
fn live_files_are_bounded_and_drift_ack_is_atomic_persistent_and_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let path = root.path().join("a.rs");
    std::fs::write(&path, "first\nold\nlast\n").unwrap();
    let dbpath = db.path().join("drift.duckdb");
    let store = Store::open(&dbpath).unwrap();
    seed(&store, &path.display().to_string(), "first\nold\nlast\n");
    let q = SourceQuery {
        document_id: Some("a".into()),
        ..Default::default()
    };
    let roots = vec![root.path().to_path_buf()];
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_freshness(&q, &roots)
            .unwrap()["all_checked_unchanged"],
        true
    );
    std::fs::write(&path, "first\nnew 🦀\nlast\n").unwrap();
    let freshness = crate::code_index::CodeIndex::new(&store)
        .code_freshness(&q, &roots)
        .unwrap();
    assert_eq!(freshness["changed"], 1);
    let id = freshness["items"][0]["drift_event_id"]
        .as_str()
        .unwrap()
        .to_string();
    let read = crate::code_index::CodeIndex::new(&store)
        .code_live_read(
            &SourceQuery {
                document_id: Some("a".into()),
                start_line: Some(2),
                end_line: Some(2),
                max_chars: Some(3),
                ..Default::default()
            },
            &roots,
        )
        .unwrap();
    assert_eq!(read["content"], "new");
    assert_eq!(read["truncated"], true);
    assert_eq!(read["drift_event_id"], id);
    assert_eq!(
        store
            .documents()
            .get_document("a")
            .unwrap()
            .unwrap()
            .content,
        "first\nold\nlast\n"
    );
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_drift_ack(&DriftAck {
            event_ids: vec![id.clone(), "unknown".into()],
            note: "reviewed".into()
        })
        .is_err());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_drift_report(&DriftQuery::default())
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_drift_ack(&DriftAck {
                event_ids: vec![id.clone()],
                note: "reviewed".into()
            })
            .unwrap()["acknowledged"],
        1
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_drift_ack(&DriftAck {
                event_ids: vec![id.clone()],
                note: "must not overwrite".into()
            })
            .unwrap()["acknowledged"],
        0
    );
    crate::code_index::CodeIndex::new(&store)
        .code_freshness(&q, &roots)
        .unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_drift_report(&DriftQuery::default())
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    drop(store);
    let store = Store::open(&dbpath).unwrap();
    let report = crate::code_index::CodeIndex::new(&store)
        .code_drift_report(&DriftQuery {
            include_acknowledged: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(report["items"][0]["acknowledgement"], "reviewed");
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_index_history(&history::HistoryQuery::default())
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn missing_disallowed_and_malformed_sources_never_report_fresh() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let path = root.path().join("a.rs");
    std::fs::write(&path, "old").unwrap();
    let store = Store::open(&db.path().join("drift.duckdb")).unwrap();
    seed(&store, &path.display().to_string(), "old");
    let q = SourceQuery {
        document_id: Some("a".into()),
        ..Default::default()
    };
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_freshness(&q, &[])
            .unwrap()["unavailable"],
        1
    );
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_live_read(&q, &[])
        .is_err());
    let roots = vec![root.path().to_path_buf()];
    std::fs::write(&path, [0xff, 0xfe, 0]).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_freshness(&q, &roots)
            .unwrap()["unavailable"],
        1
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_freshness(&q, &roots)
            .unwrap()["items"][0]["status"],
        "missing"
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_live_read(&q, &roots)
            .unwrap()["status"],
        "missing"
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_freshness(
                &SourceQuery {
                    source: Some("stored".into()),
                    ..q
                },
                &[]
            )
            .unwrap()["all_checked_unchanged"],
        true
    );
}
#[test]
fn publication_drift_has_stable_cursors_and_ack_requires_write_role() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("drift.duckdb")).unwrap();
    seed(&store, "/p/a.rs", "fn a() {}");
    let mut doc = store.documents().get_document("a").unwrap().unwrap();
    doc.content = "fn changed() {}".into();
    store.documents().upsert_document(&doc).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    store.documents().delete_document("a").unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let first = crate::code_index::CodeIndex::new(&store)
        .code_drift_report(&DriftQuery {
            limit: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(first["has_more"], true);
    assert_eq!(first["items"][0]["kind"], "removed_from_index");
    let second = crate::code_index::CodeIndex::new(&store)
        .code_drift_report(&DriftQuery {
            limit: Some(1),
            before_seq: first["next_before_seq"].as_i64(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(second["items"][0]["kind"], "modified");
    assert!(!crate::access::AccessRole::Read.allows_tool("drift_ack"));
    assert!(crate::access::AccessRole::Write.allows_tool("drift_ack"));
}
