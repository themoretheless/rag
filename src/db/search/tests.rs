use super::*;
use crate::models::Chunk;
use crate::models::Document;
use chrono::Utc;

fn sample_hit(chunk_id: &str, doc_id: &str, score: f32) -> SearchHit {
    SearchHit {
        chunk_id: chunk_id.into(),
        document_id: doc_id.into(),
        document_title: format!("title-{doc_id}"),
        document_uri: format!("uri://{doc_id}"),
        chunk_index: 0,
        content: format!("content of {chunk_id}"),
        score,
        score_vec: Some(score),
        score_lex: None,
        score_rrf: None,
        snippet: None,
        char_start: Some(0),
        char_end: Some(10),
        heading_path: None,
        section: None,
        context: None,
        explanation: None,
        rank_vec: None,
        rank_lex: None,
    }
}

fn hit_from_chunk(chunk: &Chunk, score: f32, title: &str, uri: &str) -> SearchHit {
    SearchHit {
        chunk_id: chunk.id.clone(),
        document_id: chunk.document_id.clone(),
        document_title: title.to_string(),
        document_uri: uri.to_string(),
        chunk_index: chunk.chunk_index,
        content: chunk.content.clone(),
        score,
        score_vec: Some(score),
        char_start: Some(chunk.char_start),
        char_end: Some(chunk.char_end),
        ..Default::default()
    }
}

#[test]
fn ranking_order_matches_cosine() {
    let q = [1.0f32, 0.0, 0.0];
    let a = [1.0f32, 0.0, 0.0];
    let b = [0.0f32, 1.0, 0.0];
    let c = [-1.0f32, 0.0, 0.0];

    let mut scores = [
        (cosine_similarity(&q, &b), "b"),
        (cosine_similarity(&q, &c), "c"),
        (cosine_similarity(&q, &a), "a"),
    ];
    scores.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));

    assert_eq!(scores[0].1, "a");
    assert_eq!(scores[1].1, "b");
    assert_eq!(scores[2].1, "c");
    assert!((scores[0].0 - 1.0).abs() < 1e-5);
}

#[test]
fn search_hit_fields_populated() {
    let chunk = Chunk {
        id: "c1".into(),
        document_id: "d1".into(),
        chunk_index: 2,
        content: "hello".into(),
        embedding: vec![1.0, 0.0],
        char_start: 0,
        char_end: 5,
        metadata_json: "{}".into(),
    };
    let hit = hit_from_chunk(&chunk, 0.9, "Title", "uri://x");
    assert_eq!(hit.chunk_id, "c1");
    assert_eq!(hit.document_title, "Title");
    assert_eq!(hit.chunk_index, 2);
    assert!((hit.score - 0.9).abs() < 1e-6);
}

#[test]
fn rrf_prefers_items_high_in_both_lists() {
    // a ranks 1 in both lists → highest RRF
    // b ranks 1 vec / 3 lex
    // c ranks 2 vec / 2 lex
    let vec_list = vec![
        sample_hit("a", "d1", 0.9),
        sample_hit("c", "d2", 0.8),
        sample_hit("b", "d3", 0.7),
    ];
    let mut lex_a = sample_hit("a", "d1", 5.0);
    lex_a.score_vec = None;
    lex_a.score_lex = Some(5.0);
    let mut lex_c = sample_hit("c", "d2", 4.0);
    lex_c.score_vec = None;
    lex_c.score_lex = Some(4.0);
    let mut lex_b = sample_hit("b", "d3", 3.0);
    lex_b.score_vec = None;
    lex_b.score_lex = Some(3.0);
    let lex_list = vec![lex_a, lex_c, lex_b];

    let fused = fuse_rrf(&vec_list, &lex_list, 60.0);
    assert_eq!(fused[0].chunk_id, "a");
    assert!(fused[0].score_rrf.is_some());
    assert!(fused[0].score_vec.is_some());
    assert!(fused[0].score_lex.is_some());
    // RRF(a) = 1/(60+1) + 1/(60+1)
    let expected_a = 2.0 / 61.0;
    assert!((fused[0].score_rrf.unwrap() - expected_a).abs() < 1e-6);
    assert!((fused[0].score - expected_a).abs() < 1e-6);
}

#[test]
fn rrf_includes_items_present_in_only_one_list() {
    let vec_list = vec![sample_hit("only_vec", "d1", 0.99)];
    let mut lex = sample_hit("only_lex", "d2", 10.0);
    lex.score_vec = None;
    lex.score_lex = Some(10.0);
    let lex_list = vec![lex];

    let fused = fuse_rrf(&vec_list, &lex_list, 60.0);
    assert_eq!(fused.len(), 2);
    let ids: Vec<&str> = fused.iter().map(|h| h.chunk_id.as_str()).collect();
    assert!(ids.contains(&"only_vec"));
    assert!(ids.contains(&"only_lex"));
}

#[test]
fn collapse_limits_per_document() {
    let hits = vec![
        sample_hit("c1", "d1", 1.0),
        sample_hit("c2", "d1", 0.9),
        sample_hit("c3", "d1", 0.8),
        sample_hit("c4", "d2", 0.7),
        sample_hit("c5", "d2", 0.6),
    ];
    let out = collapse_by_document(hits, 2);
    assert_eq!(out.len(), 4);
    assert_eq!(out.iter().filter(|h| h.document_id == "d1").count(), 2);
    assert_eq!(out.iter().filter(|h| h.document_id == "d2").count(), 2);
    // Order preserved for kept items
    assert_eq!(out[0].chunk_id, "c1");
    assert_eq!(out[1].chunk_id, "c2");
    assert_eq!(out[2].chunk_id, "c4");
}

#[test]
fn min_score_filters() {
    let hits = vec![
        sample_hit("a", "d1", 0.9),
        sample_hit("b", "d1", 0.4),
        sample_hit("c", "d1", 0.1),
    ];
    let out = apply_min_score(hits, 0.5);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].chunk_id, "a");
}

#[test]
fn snippet_around_query_term() {
    let content = "alpha beta gamma delta epsilon zeta eta theta";
    let snip = make_snippet(content, Some("gamma"), 20).expect("snippet");
    assert!(snip.to_lowercase().contains("gamma"));
    // Should not be longer than max_chars + ellipsis markers.
    assert!(snip.chars().count() <= 20 + 2);
}

#[test]
fn snippet_survives_case_folding_that_changes_byte_length() {
    // `İ` lowercases to two chars (three bytes more than one ASCII byte), so a
    // match offset in the lowered text is not a char boundary in the original.
    let content = format!("{}İİİ needle tail", "İ".repeat(40));
    let snip = make_snippet(&content, Some("needle"), 12).expect("snippet");
    assert!(snip.contains("needle"), "{snip}");
}

#[test]
fn snippet_prefix_without_query() {
    let content = "hello world this is a long piece of text for testing";
    let snip = make_snippet(content, None, 11).expect("snippet");
    assert!(snip.starts_with("hello world") || snip.starts_with("hello"));
}

#[test]
fn tokenize_and_tf_score() {
    let terms = tokenize("Hello, WORLD! hello");
    assert_eq!(terms, vec!["hello", "world", "hello"]);
    let q = tokenize("hello world");
    let s = tf_score(&q, "hello hello world foo");
    assert!((s - 3.0).abs() < 1e-6); // hello x2 + world x1
}

#[test]
fn sanitize_stemmer_rejects_injection() {
    assert_eq!(fts::sanitize_stemmer("porter"), "porter");
    assert_eq!(fts::sanitize_stemmer("none"), "none");
    assert_eq!(fts::sanitize_stemmer("  German "), "german");
    assert_eq!(fts::sanitize_stemmer("x'; DROP TABLE"), "porter");
}

#[test]
fn search_lex_tf_and_hybrid_with_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("search_test.duckdb");
    let store = Store::open(&path).expect("open");

    let now = Utc::now();
    let doc1 = Document {
        id: "d1".into(),
        uri: "uri://d1".into(),
        title: "Cats".into(),
        content: "cats and kittens play".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    let doc2 = Document {
        id: "d2".into(),
        uri: "uri://d2".into(),
        title: "Dogs".into(),
        content: "dogs and puppies run".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    store.upsert_document(&doc1).unwrap();
    store.upsert_document(&doc2).unwrap();

    let chunks = vec![
        Chunk {
            id: "c1".into(),
            document_id: "d1".into(),
            chunk_index: 0,
            content: "unique xylophone cats meow loudly".into(),
            embedding: vec![1.0, 0.0, 0.0],
            char_start: 0,
            char_end: 32,
            metadata_json: "{}".into(),
        },
        Chunk {
            id: "c2".into(),
            document_id: "d1".into(),
            chunk_index: 1,
            content: "more cats again cats cats".into(),
            embedding: vec![0.9, 0.1, 0.0],
            char_start: 33,
            char_end: 58,
            metadata_json: "{}".into(),
        },
        Chunk {
            id: "c3".into(),
            document_id: "d2".into(),
            chunk_index: 0,
            content: "dogs bark at the moon".into(),
            embedding: vec![0.0, 1.0, 0.0],
            char_start: 0,
            char_end: 21,
            metadata_json: "{}".into(),
        },
    ];
    store.insert_chunks(&chunks).unwrap();

    // Pure TF path (bypass FTS by calling search_lex_tf).
    let lex = search_lex_tf(&store, "cats xylophone", 10, None).unwrap();
    assert!(!lex.is_empty());
    assert!(lex[0].score_lex.is_some());
    // c1 has both cats + xylophone; c2 has many cats only
    assert!(lex.iter().any(|h| h.chunk_id == "c1"));

    // Vec mode via public search.
    let vec_hits = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 5,
            query_embedding: Some(vec![1.0, 0.0, 0.0]),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert!(!vec_hits.is_empty());
    assert!(vec_hits[0].score_vec.is_some());
    assert_eq!(vec_hits[0].chunk_id, "c1");

    // Hybrid: needs both embedding and text.
    let hybrid = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 5,
            query_text: Some("cats xylophone".into()),
            query_embedding: Some(vec![1.0, 0.0, 0.0]),
            max_chunks_per_document: Some(1),
            min_score: None,
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert!(!hybrid.is_empty());
    assert!(hybrid[0].score_rrf.is_some());
    // Diversity: at most one hit per document
    let mut seen = std::collections::HashSet::new();
    for h in &hybrid {
        assert!(seen.insert(h.document_id.clone()));
        assert!(h.snippet.is_some());
    }

    // min_score filters high threshold on vec scores (~1.0 max)
    let filtered = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 5,
            query_embedding: Some(vec![1.0, 0.0, 0.0]),
            min_score: Some(0.99),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    for h in &filtered {
        assert!(h.score >= 0.99);
    }

    // Backward-compatible search_chunks still works.
    let legacy = search_chunks(&store, &[1.0, 0.0, 0.0], 3, None).unwrap();
    assert!(!legacy.is_empty());
    assert!(legacy[0].score_vec.is_some());
}

#[test]
fn recency_half_life_boosts_newer_document() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("recency.duckdb")).unwrap();
    let now = Utc::now();
    for (id, age_days) in [("old", 120), ("new", 0)] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("uri://{id}"),
                title: id.into(),
                content: "same relevance".into(),
                metadata_json: "{}".into(),
                created_at: now - chrono::Duration::days(age_days),
                updated_at: now - chrono::Duration::days(age_days),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("chunk-{id}"),
                document_id: id.into(),
                chunk_index: 0,
                content: "same relevance".into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: 14,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
    let hits = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 2,
            query_embedding: Some(vec![1.0, 0.0]),
            recency_half_life_days: Some(30.0),
            now: Some(now),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits[0].document_id, "new");
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn search_vec_respects_wing_room_source_and_archived() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("search_scope.duckdb");
    let store = Store::open(&path).expect("open");
    let now = Utc::now();

    let active = Document {
        id: "active".into(),
        uri: "uri://active".into(),
        title: "Active".into(),
        content: "scoped active body".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some("research".into()),
        room: Some("rag".into()),
        source_file: Some("/vault/active.md".into()),
        layer: "raw".into(),
        status: "active".into(),
        ..Default::default()
    };
    let other_wing = Document {
        id: "other".into(),
        uri: "uri://other".into(),
        title: "Other".into(),
        content: "other wing body".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some("ops".into()),
        room: Some("runbooks".into()),
        source_file: Some("/vault/other.md".into()),
        layer: "wiki".into(),
        status: "active".into(),
        ..Default::default()
    };
    let archived = Document {
        id: "arch".into(),
        uri: "uri://arch".into(),
        title: "Archived".into(),
        content: "archived body".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some("research".into()),
        room: Some("rag".into()),
        source_file: Some("/vault/arch.md".into()),
        status: "archived".into(),
        ..Default::default()
    };
    store.upsert_document(&active).unwrap();
    store.upsert_document(&other_wing).unwrap();
    store.upsert_document(&archived).unwrap();

    let emb = vec![1.0f32, 0.0, 0.0];
    store
        .insert_chunks(&[
            Chunk {
                id: "c-active".into(),
                document_id: "active".into(),
                chunk_index: 0,
                content: "scoped active body".into(),
                embedding: emb.clone(),
                char_start: 0,
                char_end: 18,
                metadata_json: "{}".into(),
            },
            Chunk {
                id: "c-other".into(),
                document_id: "other".into(),
                chunk_index: 0,
                content: "other wing body".into(),
                embedding: emb.clone(),
                char_start: 0,
                char_end: 15,
                metadata_json: "{}".into(),
            },
            Chunk {
                id: "c-arch".into(),
                document_id: "arch".into(),
                chunk_index: 0,
                content: "archived body".into(),
                embedding: emb.clone(),
                char_start: 0,
                char_end: 13,
                metadata_json: "{}".into(),
            },
        ])
        .unwrap();

    // Selective scopes materialize only their matching chunks and never
    // populate the database-wide snapshot cache. This is stronger than
    // merely filtering a global ranking after it has been built.
    let scoped_cases = [
        (
            LexFilters {
                document_id: Some("active".into()),
                ..Default::default()
            },
            vec!["c-active"],
        ),
        (
            LexFilters {
                wing: Some("research".into()),
                ..Default::default()
            },
            vec!["c-active"],
        ),
        (
            LexFilters {
                room: Some("rag".into()),
                ..Default::default()
            },
            vec!["c-active"],
        ),
        (
            LexFilters {
                uri: Some("uri://other".into()),
                ..Default::default()
            },
            vec!["c-other"],
        ),
        (
            LexFilters {
                source_file: Some("/vault/other.md".into()),
                ..Default::default()
            },
            vec!["c-other"],
        ),
    ];
    for (filters, expected) in scoped_cases {
        let (snapshot, documents) = vector_search_inputs(&store, &filters).unwrap();
        let chunk_ids = snapshot
            .chunks
            .iter()
            .map(|chunk| chunk.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(chunk_ids, expected);
        assert_eq!(documents.len(), expected.len());
    }
    assert!(
        !vector_snapshots().lock().unwrap().contains_key(&path),
        "scoped searches must not populate the global snapshot cache"
    );

    // Default: exclude archived; no wing filter → active + other.
    let open = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 10,
            query_embedding: Some(emb.clone()),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    let ids: Vec<_> = open.iter().map(|h| h.document_id.as_str()).collect();
    assert!(ids.contains(&"active"));
    assert!(ids.contains(&"other"));
    assert!(!ids.contains(&"arch"));
    assert_eq!(
        vector_snapshots()
            .lock()
            .unwrap()
            .get(&path)
            .unwrap()
            .chunks
            .len(),
        3,
        "the implicit active-status filter alone must retain the global cache path"
    );

    // With a current global snapshot in memory a scoped read reuses it
    // instead of re-parsing embeddings; the document lookup still scopes it.
    let (scoped_after_global, scoped_documents) = vector_search_inputs(
        &store,
        &LexFilters {
            wing: Some("research".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(scoped_after_global.chunks.len(), 3);
    assert_eq!(
        scoped_documents.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["active"]
    );
    assert_eq!(
        vector_snapshots()
            .lock()
            .unwrap()
            .get(&path)
            .unwrap()
            .chunks
            .len(),
        3,
        "a scoped read must not replace the global snapshot"
    );

    // Wing filter.
    let wing = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 10,
            query_embedding: Some(emb.clone()),
            wing: Some("research".into()),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(wing.len(), 1);
    assert_eq!(wing[0].document_id, "active");

    // source_file filter.
    let by_src = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 10,
            query_embedding: Some(emb.clone()),
            source_file: Some("/vault/other.md".into()),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(by_src.len(), 1);
    assert_eq!(by_src[0].document_id, "other");

    let by_layer = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 10,
            query_embedding: Some(emb.clone()),
            layer: Some("raw".into()),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(by_layer.len(), 1);
    assert_eq!(by_layer[0].document_id, "active");

    // include_archived + wing.
    let with_arch = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 10,
            query_embedding: Some(emb),
            wing: Some("research".into()),
            room: Some("rag".into()),
            include_archived: true,
            ..SearchQuery::default()
        },
    )
    .unwrap();
    let arch_ids: std::collections::HashSet<_> =
        with_arch.iter().map(|h| h.document_id.as_str()).collect();
    assert!(arch_ids.contains("active"));
    assert!(arch_ids.contains("arch"));
    assert!(!arch_ids.contains("other"));
}

#[test]
fn scoped_vector_search_ranks_inside_scope_before_top_k() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("scoped_top_k.duckdb")).expect("open");
    let now = Utc::now();

    let target = Document {
        id: "target".into(),
        uri: "uri://target".into(),
        title: "Target".into(),
        content: "project result".into(),
        wing: Some("wanted".into()),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    store.upsert_document(&target).unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "chunk-target".into(),
            document_id: target.id.clone(),
            chunk_index: 0,
            content: target.content.clone(),
            embedding: vec![0.0, 1.0],
            char_start: 0,
            char_end: 14,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    // More out-of-scope perfect matches than the candidate pool. The old
    // global-top-k-then-filter path returned no result for wing="wanted".
    for index in 0..55 {
        let id = format!("other-{index:02}");
        let document = Document {
            id: id.clone(),
            uri: format!("uri://{id}"),
            title: id.clone(),
            content: "global result".into(),
            wing: Some("other".into()),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        store.upsert_document(&document).unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("chunk-{id}"),
                document_id: id,
                chunk_index: 0,
                content: document.content,
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: 13,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }

    let hits = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: 1,
            query_embedding: Some(vec![1.0, 0.0]),
            wing: Some("wanted".into()),
            timeout_ms: None,
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document_id, "target");
    assert_eq!(hits[0].rank_vec, Some(1));

    // Hybrid must use the same scoped vector path, then retain both source
    // ranks and expose its stage diagnostics on the fused hit.
    let hybrid = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 1,
            query_text: Some("project result".into()),
            query_embedding: Some(vec![1.0, 0.0]),
            wing: Some("wanted".into()),
            rrf_k: 37.0,
            timeout_ms: None,
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hybrid.len(), 1);
    assert_eq!(hybrid[0].document_id, "target");
    assert_eq!(hybrid[0].rank_vec, Some(1));
    assert_eq!(hybrid[0].rank_lex, Some(1));
    let explanation = hybrid[0].explanation.as_ref().unwrap();
    assert!(explanation.vec_ms.is_some());
    assert!(explanation.lex_ms.is_some());
    assert_eq!(explanation.rrf_k, Some(37.0));
}

#[test]
fn vector_snapshot_refreshes_after_chunk_generation_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("vector-generation.duckdb");
    let store = Store::open(&path).expect("open");
    let document = Document {
        id: "doc".into(),
        uri: "uri://doc".into(),
        title: "Document".into(),
        content: "first".into(),
        ..Default::default()
    };
    store.upsert_document(&document).unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "before".into(),
            document_id: document.id.clone(),
            chunk_index: 0,
            content: "before content".into(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: 14,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let first = search_chunks(&store, &[1.0, 0.0], 1, None).unwrap();
    assert_eq!(first[0].chunk_id, "before");
    let first_generation = {
        let conn = store.lock().unwrap();
        fts::chunks_generation(&conn).unwrap()
    };

    store
        .replace_chunks_for_document(
            &document.id,
            &[Chunk {
                id: "after".into(),
                document_id: document.id.clone(),
                chunk_index: 0,
                content: "after content".into(),
                embedding: vec![0.0, 1.0],
                char_start: 0,
                char_end: 13,
                metadata_json: "{}".into(),
            }],
        )
        .unwrap();
    let second_generation = {
        let conn = store.lock().unwrap();
        fts::chunks_generation(&conn).unwrap()
    };
    assert!(second_generation > first_generation);

    let second = search_chunks(&store, &[0.0, 1.0], 1, None).unwrap();
    assert_eq!(second[0].chunk_id, "after");
    assert_eq!(second[0].content, "after content");
    let cached_generation = vector_snapshots()
        .lock()
        .unwrap()
        .get(&path)
        .unwrap()
        .generation;
    assert_eq!(cached_generation, second_generation);
}

#[test]
fn vector_snapshot_refreshes_after_embedding_only_update() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("vector-embedding-only-generation.duckdb");
    let store = Store::open(&path).expect("open");
    for (document_id, chunk_id, embedding) in [
        ("a-target", "chunk-target", vec![1.0, 0.0]),
        ("z-rival", "chunk-rival", vec![0.0, 1.0]),
    ] {
        store
            .upsert_document(&Document {
                id: document_id.into(),
                uri: format!("uri://{document_id}"),
                title: document_id.into(),
                content: "unchanged searchable text".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: chunk_id.into(),
                document_id: document_id.into(),
                chunk_index: 0,
                content: "unchanged searchable text".into(),
                embedding,
                char_start: 0,
                char_end: 25,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }

    let before = search_chunks(&store, &[0.0, 1.0], 2, None).unwrap();
    assert_eq!(before[0].document_id, "z-rival");
    let cached_before = vector_snapshots()
        .lock()
        .unwrap()
        .get(&path)
        .unwrap()
        .generation;

    let document = store.get_document("a-target").unwrap().unwrap();
    let chunks = store.list_chunks_for_document(&document.id).unwrap();
    store
        .update_chunk_embeddings_atomic(
            &document.id,
            document.revision,
            &chunks,
            &[vec![0.0, 1.0]],
            None,
        )
        .unwrap();

    let after = search_chunks(&store, &[0.0, 1.0], 2, None).unwrap();
    assert_eq!(after[0].document_id, "a-target");
    let cached_after = vector_snapshots()
        .lock()
        .unwrap()
        .get(&path)
        .unwrap()
        .generation;
    assert!(cached_after > cached_before);
}

#[tokio::test]
async fn active_corpus_mutation_rejects_all_search_modes_and_releases_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("hybrid-source-sync-busy.duckdb");
    let store = Store::open(&path).expect("open");
    let document = Document {
        id: "project-doc".into(),
        uri: "uri://project-doc".into(),
        title: "Project document".into(),
        content: "needle".into(),
        wing: Some("project".into()),
        ..Default::default()
    };
    store.upsert_document(&document).unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "project-chunk".into(),
            document_id: document.id.clone(),
            chunk_index: 0,
            content: document.content.clone(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: 6,
            metadata_json: "{}".into(),
        }])
        .unwrap();
    store.ensure_fts("porter").unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "project-chunk-new".into(),
            document_id: document.id.clone(),
            chunk_index: 1,
            content: "fresh needle".into(),
            embedding: vec![1.0, 0.0],
            char_start: 7,
            char_end: 19,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let lifecycle_before = {
        let conn = store.lock().unwrap();
        fts::fts_generation_state(&conn).unwrap()
    };
    assert!(lifecycle_before.dirty);

    let sync_guard = store.corpus_mutation_lane().write_owned().await;
    let hybrid_query = SearchQuery {
        mode: SearchMode::Hybrid,
        top_k: 5,
        query_text: Some("needle".into()),
        query_embedding: Some(vec![1.0, 0.0]),
        wing: Some("project".into()),
        ..SearchQuery::default()
    };
    let lex_query = SearchQuery {
        mode: SearchMode::Lex,
        query_text: Some("needle".into()),
        wing: Some("project".into()),
        ..SearchQuery::default()
    };
    let vec_query = SearchQuery {
        mode: SearchMode::Vec,
        top_k: 5,
        query_embedding: Some(vec![1.0, 0.0]),
        wing: Some("project".into()),
        ..SearchQuery::default()
    };
    for query in [&lex_query, &vec_query, &hybrid_query] {
        let error = search(&store, query).unwrap_err();
        let AppError::Busy(message) = error else {
            panic!("expected retryable busy error, got {error}");
        };
        assert_eq!(
            message,
            "exclusive corpus mutation is active; retry search after it completes"
        );
        assert!(!message.contains("http"));
        assert!(!message.contains(&path.display().to_string()));
    }
    let lifecycle_while_busy = {
        let conn = store.lock().unwrap();
        fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(lifecycle_while_busy, lifecycle_before);
    assert!(!vector_snapshots().lock().unwrap().contains_key(&path));

    drop(sync_guard);
    let hits = search(&store, &hybrid_query).unwrap();
    assert!(hits.iter().any(|hit| hit.chunk_id == "project-chunk-new"));
    let lifecycle_after = {
        let conn = store.lock().unwrap();
        fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!lifecycle_after.dirty);
    assert_eq!(
        lifecycle_after.rebuild_count,
        lifecycle_before.rebuild_count + 1
    );
}

#[test]
fn query_guards_and_multi_rrf_are_deterministic() {
    let empty = SearchQuery {
        mode: SearchMode::Lex,
        query_text: Some("  ".into()),
        ..SearchQuery::default()
    };
    assert!(validate_query(&empty)
        .unwrap_err()
        .to_string()
        .contains("must not be empty"));
    let too_long = SearchQuery {
        mode: SearchMode::Lex,
        query_text: Some("x".repeat(MAX_QUERY_CHARS + 1)),
        ..SearchQuery::default()
    };
    assert!(validate_query(&too_long).is_err());
    let too_many = SearchQuery {
        top_k: MAX_TOP_K + 1,
        ..SearchQuery::default()
    };
    assert!(validate_query(&too_many)
        .unwrap_err()
        .to_string()
        .contains("hard cap"));

    let lists = vec![
        vec![sample_hit("b", "d2", 1.0), sample_hit("a", "d1", 1.0)],
        vec![sample_hit("a", "d1", 1.0), sample_hit("b", "d2", 1.0)],
    ];
    let first = fuse_rrf_many(&lists, DEFAULT_RRF_K, 10);
    let second = fuse_rrf_many(&lists, DEFAULT_RRF_K, 10);
    assert_eq!(
        first.iter().map(|h| &h.chunk_id).collect::<Vec<_>>(),
        second.iter().map(|h| &h.chunk_id).collect::<Vec<_>>()
    );
    assert_eq!(first[0].chunk_id, "a");
    assert!(first[0]
        .explanation
        .as_ref()
        .unwrap()
        .reasons
        .contains(&"multi_query_rrf".into()));
}

#[test]
fn field_boosts_explain_title_uri_heading_pin_and_document_boost() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("boosts.duckdb")).unwrap();
    let document = Document {
        id: "d".into(),
        uri: "rag://exact-topic".into(),
        title: "Exact Topic".into(),
        content: "body".into(),
        metadata_json: "{}".into(),
        pinned: true,
        boost: 2.0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..Default::default()
    };
    store.upsert_document(&document).unwrap();
    let mut hits = vec![SearchHit {
        chunk_id: "c".into(),
        document_id: "d".into(),
        document_title: document.title.clone(),
        document_uri: document.uri.clone(),
        chunk_index: 0,
        content: "body".into(),
        score: 1.0,
        heading_path: Some(vec!["Exact Topic".into()]),
        ..Default::default()
    }];
    let ranking = ranking_fields(&store, &hits).unwrap();
    apply_field_boosts(&ranking, &mut hits, Some("Exact Topic"));
    let reasons = &hits[0].explanation.as_ref().unwrap().reasons;
    for expected in [
        "exact_title_boost",
        "uri_match_boost",
        "heading_match_boost",
        "pinned_document_boost",
        "document_boost",
    ] {
        assert!(
            reasons.iter().any(|reason| reason == expected),
            "missing {expected}"
        );
    }
    assert!(hits[0].score > 3.0);
}

#[test]
fn search_context_budget_includes_expansion_and_zero_returns_no_hits() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("context-budget.duckdb")).unwrap();
    let doc = Document {
        id: "budget-doc".into(),
        uri: "wiki://budget".into(),
        title: "Budget document".into(),
        content: "expanded context".into(),
        ..Default::default()
    };
    store.upsert_document(&doc).unwrap();
    let chunks: Vec<Chunk> = (0..3)
        .map(|i| Chunk {
            id: format!("budget-chunk-{i}"),
            document_id: doc.id.clone(),
            chunk_index: i,
            content: "Русский expanded context 🦀 ".repeat(1_000),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: 100,
            metadata_json: r#"{"heading_path":["Budget"]}"#.into(),
        })
        .collect();
    store.insert_chunks(&chunks).unwrap();
    for budget in [0, 1, 32] {
        let query = SearchQuery {
            top_k: 1,
            query_embedding: Some(vec![1.0, 0.0]),
            mode: SearchMode::Vec,
            max_context_tokens: Some(budget),
            context_expansion: Some(ContextExpansion::ParentSection),
            ..Default::default()
        };
        let hits = search(&store, &query).unwrap();
        let block = crate::search_pack::format_context_block(&hits);
        assert!(crate::search_pack::estimate_tokens(&block) <= budget);
        if budget < 32 {
            assert!(hits.is_empty());
        } else {
            assert_eq!(hits.len(), 1);
            assert!(hits[0].context.is_none());
            assert!(hits[0].snippet.is_none());
        }
    }
}

#[test]
fn diversity_refill_recovers_second_document_in_every_mode_without_scope_leaks() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("diversity-refill.duckdb")).unwrap();
    for (id, wing, count) in [
        ("dominant", "project", 60),
        ("second", "project", 1),
        ("foreign", "other", 80),
    ] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                title: id.into(),
                wing: Some(wing.into()),
                content: "needle".into(),
                ..Default::default()
            })
            .unwrap();
        let chunks: Vec<Chunk> = (0..count)
            .map(|index| Chunk {
                id: format!("{id}-{index}"),
                document_id: id.into(),
                chunk_index: index,
                content: if id == "second" {
                    "needle distant unrelated body".into()
                } else {
                    "needle needle needle needle".into()
                },
                embedding: if id == "second" {
                    vec![0.8, 0.6]
                } else {
                    vec![1.0, 0.0]
                },
                char_start: 0,
                char_end: 26,
                metadata_json: "{}".into(),
            })
            .collect();
        store.insert_chunks(&chunks).unwrap();
    }
    for mode in [SearchMode::Vec, SearchMode::Lex, SearchMode::Hybrid] {
        let query = SearchQuery {
            mode,
            top_k: 2,
            query_text: Some("needle".into()),
            query_embedding: Some(vec![1.0, 0.0]),
            wing: Some("project".into()),
            max_chunks_per_document: Some(1),
            timeout_ms: Some(10_000),
            ..Default::default()
        };
        let hits = search(&store, &query).unwrap();
        assert_eq!(hits.len(), 2, "mode={mode:?}");
        let ids: HashSet<_> = hits.iter().map(|hit| hit.document_id.as_str()).collect();
        assert_eq!(ids, HashSet::from(["dominant", "second"]), "mode={mode:?}");
        assert!(
            hits.iter().all(|hit| hit
                .explanation
                .as_ref()
                .unwrap()
                .reasons
                .contains(&"diversity_candidate_refill".into())),
            "mode={mode:?}"
        );
    }
}

#[test]
fn diversity_refill_capacity_ignores_duplicate_chunks_and_min_score_rejections() {
    let hits = vec![
        sample_hit("a", "one", 1.0),
        sample_hit("a", "one", 1.0),
        sample_hit("b", "one", 0.9),
        sample_hit("c", "two", 0.1),
    ];
    let query = SearchQuery {
        max_chunks_per_document: Some(1),
        min_score: Some(0.5),
        ..Default::default()
    };
    assert_eq!(diversity_capacity(&hits, &query), Some(1));
    assert_eq!(diversity_capacity(&hits, &SearchQuery::default()), None);
}

#[test]
fn diversity_refill_stops_at_bound_with_an_explicit_reason() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("diversity-cap.duckdb")).unwrap();
    store
        .upsert_document(&Document {
            id: "dominant".into(),
            uri: "doc://dominant".into(),
            title: "Dominant".into(),
            content: "needle".into(),
            ..Default::default()
        })
        .unwrap();
    let chunks: Vec<Chunk> = (0..MAX_DIVERSITY_CANDIDATES + 1)
        .map(|i| Chunk {
            id: format!("cap-{i}"),
            document_id: "dominant".into(),
            chunk_index: i as i32,
            content: "needle".into(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: 6,
            metadata_json: "{}".into(),
        })
        .collect();
    store.insert_chunks(&chunks).unwrap();
    let query = SearchQuery {
        top_k: 2,
        query_embedding: Some(vec![1.0, 0.0]),
        max_chunks_per_document: Some(1),
        // Not a promptness test: the 4 097-chunk scan outgrows any fixed wall on CI hardware.
        timeout_ms: None,
        ..Default::default()
    };
    let hits = search(&store, &query).unwrap();
    assert_eq!(hits.len(), 1);
    let explanation = hits[0].explanation.as_ref().unwrap();
    assert!(explanation
        .reasons
        .contains(&"diversity_candidate_cap_reached".into()));
    assert!(explanation.vec_ms.unwrap() > 0.0);
    let scoped = search(
        &store,
        &SearchQuery {
            document_id: Some("dominant".into()),
            ..query
        },
    )
    .unwrap();
    assert!(!scoped[0]
        .explanation
        .as_ref()
        .unwrap()
        .reasons
        .contains(&"diversity_candidate_refill".into()));
}

#[test]
fn scoped_vector_cache_sees_documents_moved_into_the_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("scoped-cache.duckdb")).unwrap();
    let now = Utc::now();
    for (id, wing) in [("inside", "research"), ("outside", "ops")] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("uri://{id}"),
                title: id.into(),
                content: format!("{id} body"),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                wing: Some(wing.into()),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("c-{id}"),
                document_id: id.into(),
                chunk_index: 0,
                content: format!("{id} body"),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: 10,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
    let scoped = |store: &Store| {
        let mut ids: Vec<String> = search(
            store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 10,
                query_embedding: Some(vec![1.0, 0.0]),
                wing: Some("research".into()),
                ..SearchQuery::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|hit| hit.document_id)
        .collect();
        ids.sort();
        ids
    };
    assert_eq!(scoped(&store), vec!["inside"]);

    // A metadata-only move does not advance the chunk generation.
    store
        .update_document_placement(
            "outside",
            &crate::models::PlacementUpdate {
                wing: Some("research".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .expect("moved");

    assert_eq!(scoped(&store), vec!["inside", "outside"]);
}

#[test]
fn refreshed_vector_snapshot_matches_a_full_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("snapshot-refresh.duckdb")).unwrap();
    let now = Utc::now();
    let add = |id: &str, vectors: &[[f32; 2]]| {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("uri://{id}"),
                title: id.into(),
                content: format!("{id} body"),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                ..Default::default()
            })
            .unwrap();
        let chunks: Vec<Chunk> = vectors
            .iter()
            .enumerate()
            .map(|(index, vector)| Chunk {
                id: format!("{id}-{index}"),
                document_id: id.into(),
                chunk_index: index as i32,
                content: format!("{id} chunk {index}"),
                embedding: vector.to_vec(),
                char_start: 0,
                char_end: 5,
                metadata_json: "{}".into(),
            })
            .collect();
        store.replace_chunks_for_document(id, &chunks).unwrap();
    };
    for index in 0..6 {
        add(
            &format!("doc{index}"),
            &[[1.0, index as f32], [index as f32, 1.0]],
        );
    }
    let previous = {
        let conn = store.lock().unwrap();
        build_vector_snapshot(&conn, 1).unwrap()
    };

    // New document, replaced chunks, and an in-place embedding change.
    add("doc-new", &[[0.5, 0.5]]);
    add("doc2", &[[3.0, 4.0]]);
    {
        let conn = store.lock().unwrap();
        // Chunk writers maintain the digest alongside the JSON.
        conn.execute(
            "UPDATE chunks SET embedding_json = '[0.0,2.0]', embedding_digest = ? \
             WHERE id = 'doc4-1'",
            [crate::db::store::embedding_digest("[0.0,2.0]")],
        )
        .unwrap();
    }

    let conn = store.lock().unwrap();
    let refreshed = refresh_vector_snapshot(&conn, &previous, 2, None).unwrap();
    let rebuilt = build_vector_snapshot(&conn, 2).unwrap();
    let view = |snapshot: &VectorSnapshot| {
        snapshot
            .chunks
            .iter()
            .map(|chunk| {
                (
                    chunk.id.clone(),
                    chunk.document_id.clone(),
                    chunk.chunk_index,
                    chunk.vector.to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(view(&refreshed), view(&rebuilt));
    assert_eq!(refreshed.generation, 2);
}

#[test]
fn scoped_vector_snapshot_refresh_matches_a_scoped_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("scoped-refresh.duckdb")).unwrap();
    let now = Utc::now();
    let add = |id: &str, wing: &str, vectors: &[[f32; 2]]| {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("uri://{id}"),
                title: id.into(),
                content: format!("{id} body"),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                wing: Some(wing.into()),
                ..Default::default()
            })
            .unwrap();
        let chunks: Vec<Chunk> = vectors
            .iter()
            .enumerate()
            .map(|(index, vector)| Chunk {
                id: format!("{id}-{index}-{}", vector[0]),
                document_id: id.into(),
                chunk_index: index as i32,
                content: format!("{id} chunk {index}"),
                embedding: vector.to_vec(),
                char_start: 0,
                char_end: 5,
                metadata_json: "{}".into(),
            })
            .collect();
        store.replace_chunks_for_document(id, &chunks).unwrap();
    };
    for index in 0..8 {
        let wing = if index % 2 == 0 { "alpha" } else { "beta" };
        add(
            &format!("doc{index}"),
            wing,
            &[[1.0, index as f32], [index as f32, 1.0]],
        );
    }
    let filters = LexFilters {
        wing: Some("alpha".into()),
        ..Default::default()
    };
    let (scoped_previous, global_previous) = {
        let conn = store.lock().unwrap();
        (
            build_vector_snapshot_with_filters(&conn, 1, Some(&filters)).unwrap(),
            build_vector_snapshot(&conn, 1).unwrap(),
        )
    };
    add("doc-new", "alpha", &[[0.5, 0.5]]);
    add("doc2", "alpha", &[[3.0, 4.0]]);

    let conn = store.lock().unwrap();
    let rebuilt = build_vector_snapshot_with_filters(&conn, 2, Some(&filters)).unwrap();
    let view = |snapshot: &VectorSnapshot| {
        snapshot
            .chunks
            .iter()
            .map(|chunk| (chunk.id.clone(), chunk.vector.to_vec()))
            .collect::<Vec<_>>()
    };
    for previous in [&scoped_previous, &global_previous] {
        let refreshed = refresh_vector_snapshot(&conn, previous, 2, Some(&filters)).unwrap();
        assert_eq!(view(&refreshed), view(&rebuilt));
    }
}
