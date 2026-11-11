use super::*;
use crate::models::{Chunk, Document};
fn store(source: &str) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::testkit::open_test_store(&dir.path().join("code.duckdb")).unwrap();
    store
        .documents()
        .upsert_document(&Document {
            id: "a".into(),
            uri: "file:///p/a.py".into(),
            source_file: Some("/p/a.py".into()),
            content: source.into(),
            wing: Some("alpha".into()),
            ..Default::default()
        })
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    (dir, store)
}
fn path() -> CodeReadQuery {
    CodeReadQuery {
        selection: CodeQuery {
            path: Some("/p/a.py".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}
#[test]
fn code_read_single_window_unicode_crlf_and_budget() {
    let (_dir, s) = store("def target():\r\n    return 'αβγ'\r\n");
    let mut q = CodeReadQuery {
        selection: CodeQuery {
            symbol: Some("target".into()),
            ..Default::default()
        },
        start: Some(2),
        end: Some(2),
        ..Default::default()
    };
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["code"], "    return 'αβγ'");
    assert_eq!(r["source_ranges"][0]["start_line"], 2);
    q.max_chars = Some(13);
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["code"].as_str().unwrap().chars().count(), 13);
    assert_eq!(r["text_truncated"], true);
    q.start = Some(50);
    q.end = None;
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["outside_window"], true);
    assert_eq!(r["code"], "");
}
#[test]
fn code_read_file_nested_and_container_partition_preserves_unique_bytes() {
    let body="class C:\n    def first(self):\n        return 1\n    def second(self):\n        return 2\n\ndef tail():\n    return 3\n";
    let (_dir, s) = store(body);
    let mut q = path();
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["nested_dropped"], 2);
    assert_eq!(r["items"].as_array().unwrap().len(), 2);
    q.include_containers = true;
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["nested_dropped"], 0);
    let mut positions = BTreeSet::new();
    for item in r["items"].as_array().unwrap() {
        for span in item["segments"].as_array().unwrap() {
            let a = span["start_byte"].as_u64().unwrap() as usize;
            let b = span["end_byte"].as_u64().unwrap() as usize;
            assert_eq!(span["code"], &body[a..b]);
            for i in a..b {
                assert!(positions.insert(i), "duplicate byte {i}");
            }
        }
    }
    assert_eq!(r["items"].as_array().unwrap().len(), 4);
    q.selection.limit = Some(1);
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["spans_truncated"], true);
    assert_eq!(r["retained_spans_total"], 4);
}
#[test]
fn code_read_chunks_validate_unicode_offsets_and_remove_overlap() {
    let body = "# αβγ\ndef target():\n    return 1\n";
    let (_dir, s) = store(body);
    for (id, a, b) in [("one", 0, 17), ("two", 5, 24)] {
        s.chunks()
            .insert_chunks(&[Chunk {
                id: id.into(),
                document_id: "a".into(),
                chunk_index: 0,
                content: body.chars().skip(a).take(b - a).collect(),
                embedding: vec![],
                char_start: a as i32,
                char_end: b as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
    let q = CodeReadQuery {
        entry_ids: Some(vec!["one".into(), "two".into(), "missing".into()]),
        ..Default::default()
    };
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["unresolved_entry_ids"], serde_json::json!(["missing"]));
    assert_eq!(r["overlap_bytes_omitted"].as_u64().unwrap(), 12);
    let pieces = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|i| i["segments"].as_array().unwrap())
        .map(|s| s["code"].as_str().unwrap())
        .collect::<String>();
    assert_eq!(pieces, body.chars().take(24).collect::<String>());
}
#[test]
fn code_read_path_project_ambiguity_and_stale_exclusion() {
    let (_dir, s) = store("def a(): pass\n");
    s.documents()
        .upsert_document(&Document {
            id: "b".into(),
            uri: "file:///p/b.py".into(),
            source_file: Some("/p/a.py".into()),
            content: "def b(): pass\n".into(),
            wing: Some("beta".into()),
            ..Default::default()
        })
        .unwrap();
    crate::code_index::CodeIndex::new(&s).index_code().unwrap();
    let ambiguous = crate::code_index::CodeIndex::new(&s)
        .code_read(&path())
        .unwrap();
    assert_eq!(ambiguous["ambiguous"], true);
    assert_eq!(
        ambiguous,
        crate::code_index::CodeIndex::new(&s)
            .code_read(&path())
            .unwrap()
    );
    let mut q = path();
    q.project = Some("alpha".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap()["found"],
        true
    );
    s.documents()
        .upsert_document(&Document {
            id: "a".into(),
            uri: "file:///p/a.py".into(),
            source_file: Some("/p/a.py".into()),
            content: "def changed(): pass".into(),
            wing: Some("alpha".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap()["found"],
        false
    );
    assert_eq!(
        crate::code_index::CodeIndex::new(&s)
            .code_read(&path())
            .unwrap()["ambiguous"],
        true
    );
}
#[test]
fn code_read_validation_file_without_symbols_and_shared_budget() {
    let (_dir, s) = store("# αβγ\r\n# second\r\n");
    let mut q = path();
    q.start = Some(1);
    q.end = Some(1);
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["items"][0]["segments"][0]["code"], "# αβγ\r\n");
    q.start = Some(0);
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    q.start = None;
    q.max_chars = Some(0);
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    q.max_chars = None;
    q.source = Some("disk".into());
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    assert!(crate::code_index::CodeIndex::new(&s)
        .code_read(&CodeReadQuery::default())
        .is_err());
    let (_d, s) = store("def a(): return 'αβγ'\ndef b(): return 'δεζ'\n");
    let mut q = path();
    q.max_chars = Some(24);
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    let count = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|i| i["segments"].as_array().unwrap())
        .map(|s| s["code"].as_str().unwrap().chars().count())
        .sum::<usize>();
    assert_eq!(count, 24);
    assert_eq!(r["text_truncated"], true);
}
#[test]
fn code_read_segment_cap_and_invalid_chunks_are_explicit() {
    let source = format!(
        "class C:\n{}",
        (0..1005)
            .map(|i| format!("    def m{i}(self): pass\n"))
            .collect::<String>()
    );
    let (_dir, s) = store(&source);
    let mut q = path();
    q.include_containers = true;
    q.max_chars = Some(100000);
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["segments_truncated"], true);
    assert_eq!(r["truncated"], true);
    let segments = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["segments"].as_array().unwrap().len())
        .sum::<usize>();
    assert_eq!(segments, 1000);
    s.chunks()
        .insert_chunks(&[Chunk {
            id: "invalid".into(),
            document_id: "a".into(),
            chunk_index: 0,
            content: "wrong".into(),
            embedding: vec![],
            char_start: 0,
            char_end: 5,
            metadata_json: "{}".into(),
        }])
        .unwrap();
    let q = CodeReadQuery {
        entry_ids: Some(vec!["invalid".into()]),
        ..Default::default()
    };
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["found"], false);
    assert_eq!(r["unresolved_entry_ids"], serde_json::json!(["invalid"]));
    let q = CodeReadQuery {
        entry_ids: Some(vec!["invalid".into()]),
        project: Some("beta".into()),
        ..Default::default()
    };
    assert_eq!(
        crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap()["found"],
        false
    );
}
#[test]
fn code_cursor_single_large_line_resumes_with_changed_page_budget() {
    let body = format!("def target(): return '{}'\n", "α".repeat(120005));
    let (_dir, s) = store(&body);
    let mut q = CodeReadQuery {
        selection: CodeQuery {
            symbol: Some("target".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut text = String::new();
    let mut pages = 0;
    loop {
        let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
        pages += 1;
        assert!(pages <= 4);
        assert_eq!(r["is_continuation"], q.cursor.is_some());
        text.push_str(r["code"].as_str().unwrap());
        if r["read_complete"] == true {
            assert!(r["next_cursor"].is_null());
            break;
        }
        let next = r["next_cursor"].as_str().unwrap().to_string();
        assert_ne!(q.cursor.as_deref(), Some(next.as_str()));
        q.cursor = Some(next);
        q.max_chars = Some(100000);
    }
    assert_eq!(pages, 3);
    assert_eq!(text, body.trim_end());
}
#[test]
fn code_cursor_partially_overlapping_chunks_do_not_repeat_across_pages() {
    let body = "# αβγ\ndef target():\n    return 1\n";
    let (_dir, s) = store(body);
    for (id, a, b) in [("one", 0, 17), ("two", 5, 24)] {
        s.chunks()
            .insert_chunks(&[Chunk {
                id: id.into(),
                document_id: "a".into(),
                chunk_index: 0,
                content: body.chars().skip(a).take(b - a).collect(),
                embedding: vec![],
                char_start: a as i32,
                char_end: b as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
    let mut q = CodeReadQuery {
        entry_ids: Some(vec!["one".into(), "two".into()]),
        selection: CodeQuery {
            limit: Some(1),
            ..Default::default()
        },
        max_chars: Some(4),
        ..Default::default()
    };
    let mut text = String::new();
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
        pages += 1;
        assert!(pages < 20);
        for i in r["items"].as_array().unwrap() {
            for p in i["segments"].as_array().unwrap() {
                let a = p["start_byte"].as_u64().unwrap() as usize;
                let b = p["end_byte"].as_u64().unwrap() as usize;
                for pos in a..b {
                    assert!(seen.insert(pos));
                }
                text.push_str(p["code"].as_str().unwrap());
            }
        }
        let Some(next) = r["next_cursor"].as_str() else {
            break;
        };
        q.cursor = Some(next.into());
    }
    assert_eq!(text, body.chars().take(24).collect::<String>());
}
#[test]
fn code_cursor_byte_windows_reject_split_utf8_and_support_empty_boundary() {
    let body = "def target(): return 'αβγ'\n";
    let (_dir, s) = store(body);
    let a = body.find('α').unwrap();
    let mut q = CodeReadQuery {
        selection: CodeQuery {
            symbol: Some("target".into()),
            ..Default::default()
        },
        start_byte: Some(a),
        end_byte: Some(a + 4),
        ..Default::default()
    };
    assert_eq!(
        crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap()["code"],
        "αβ"
    );
    q.end_byte = Some(a + 1);
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    q.end_byte = Some(body.len() + 1);
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    q.start_byte = Some(a);
    q.end_byte = Some(a);
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    assert_eq!(r["code"], "");
    assert_eq!(r["read_complete"], true);
}
#[test]
fn code_cursor_changed_queries_or_stale_sources_conflict_before_lookup() {
    let (_dir, s) = store("def target(): return 'old'\n");
    let mut q = CodeReadQuery {
        selection: CodeQuery {
            symbol: Some("target".into()),
            ..Default::default()
        },
        max_chars: Some(3),
        ..Default::default()
    };
    q.cursor = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap()["next_cursor"]
        .as_str()
        .map(str::to_string);
    q.include_containers = true;
    assert!(matches!(
        crate::code_index::CodeIndex::new(&s).code_read(&q),
        Err(AppError::Conflict(_))
    ));
    q.include_containers = false;
    let original = q.cursor.clone().unwrap();
    q.cursor = Some("bad".into());
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    let mut parts = original.split(':').map(str::to_string).collect::<Vec<_>>();
    parts[3] = usize::MAX.to_string();
    q.cursor = Some(parts.join(":"));
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    q.cursor = Some(original);
    s.documents()
        .upsert_document(&Document {
            id: "a".into(),
            uri: "file:///p/a.py".into(),
            source_file: Some("/p/a.py".into()),
            content: "def target(): return 'new'\n".into(),
            wing: Some("alpha".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(
        crate::code_index::CodeIndex::new(&s).code_read(&q),
        Err(AppError::Conflict(_))
    ));
    crate::code_index::CodeIndex::new(&s).index_code().unwrap();
    assert!(matches!(
        crate::code_index::CodeIndex::new(&s).code_read(&q),
        Err(AppError::Conflict(_))
    ));
}
#[test]
fn code_cursor_delimited_ids_match_array_and_have_bounded_parsing() {
    let (_dir, s) = store("def first(): pass\ndef second(): pass\n");
    let symbols = crate::code_index::CodeIndex::new(&s)
        .code_snapshot()
        .unwrap()
        .symbols;
    let a = &symbols[0].id;
    let b = &symbols[1].id;
    let mut q: CodeReadQuery = serde_json::from_value(
        serde_json::json!({"entry_ids":format!("{a}, \n{b}"),"max_chars":2}),
    )
    .unwrap();
    let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
    let expected = r["next_cursor"].clone();
    q.cursor = r["next_cursor"].as_str().map(str::to_string);
    let mut array: CodeReadQuery =
        serde_json::from_value(serde_json::json!({"entry_ids":[b,a,a],"max_chars":2})).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&s)
            .code_read(&array)
            .unwrap()["next_cursor"],
        expected
    );
    array.cursor = q.cursor.clone();
    assert_eq!(
        crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap(),
        crate::code_index::CodeIndex::new(&s)
            .code_read(&array)
            .unwrap()
    );
    assert!(serde_json::from_value::<CodeReadQuery>(
        serde_json::json!({"entry_ids":"x".repeat(513)})
    )
    .is_err());
    let many: CodeReadQuery =
        serde_json::from_value(serde_json::json!({"entry_ids":"id ".repeat(1000)})).unwrap();
    assert_eq!(many.entry_ids.as_ref().unwrap().len(), 101);
    assert!(crate::code_index::CodeIndex::new(&s)
        .code_read(&many)
        .is_err());
}
#[test]
fn code_cursor_segment_limit_and_container_metadata_resume_without_missing_bytes() {
    let body = format!(
        "class C:\n{}",
        (0..1005)
            .map(|i| format!("    def m{i}(self): pass\n"))
            .collect::<String>()
    );
    let (_dir, s) = store(&body);
    let mut q = path();
    q.include_containers = true;
    q.max_chars = Some(100000);
    q.selection.limit = Some(500);
    let expected: BTreeSet<_> = crate::code_index::CodeIndex::new(&s)
        .code_snapshot()
        .unwrap()
        .symbols
        .iter()
        .flat_map(|s| s.start_byte..s.end_byte)
        .collect();
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    let mut previous = None;
    loop {
        let r = crate::code_index::CodeIndex::new(&s).code_read(&q).unwrap();
        pages += 1;
        assert!(pages < 6);
        for i in r["items"].as_array().unwrap() {
            for p in i["segments"].as_array().unwrap() {
                let a = p["start_byte"].as_u64().unwrap() as usize;
                let b = p["end_byte"].as_u64().unwrap() as usize;
                assert_eq!(p["code"], &body[a..b]);
                for pos in a..b {
                    assert!(seen.insert(pos), "duplicate {pos}");
                }
            }
        }
        let Some(next) = r["next_cursor"].as_str() else {
            assert_eq!(r["read_complete"], true);
            break;
        };
        assert_ne!(previous.as_deref(), Some(next));
        previous = Some(next.to_string());
        q.cursor = previous.clone();
    }
    assert_eq!(seen, expected);
}
#[test]
fn code_cursor_corrupt_child_boundary_returns_error_without_poisoning_store() {
    let body = "class C:\n    'α'\n    def child(self): pass\n";
    let (_dir, s) = store(body);
    let payload: String = s
        .lock()
        .unwrap()
        .query_row(
            "SELECT payload FROM code_index_documents WHERE document_id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut doc: IndexedDocument = serde_json::from_str(&payload).unwrap();
    doc.symbols
        .iter_mut()
        .find(|s| s.name == "child")
        .unwrap()
        .start_byte = body.find('α').unwrap() + 1;
    s.lock()
        .unwrap()
        .execute(
            "UPDATE code_index_documents SET payload=? WHERE document_id='a'",
            [serde_json::to_string(&doc).unwrap()],
        )
        .unwrap();
    let mut q = path();
    q.include_containers = true;
    assert!(crate::code_index::CodeIndex::new(&s).code_read(&q).is_err());
    assert!(crate::code_index::CodeIndex::new(&s)
        .code_snapshot()
        .is_ok());
}
