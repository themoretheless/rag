//! Timing harness for the N+1 fixes of Stage 5 item 5 (tag writes,
//! collections, KG writes, the health chunkless count). Ignored by default;
//! numbers go to docs/refactor/BASELINE.md.
//!
//! Run: `cargo test --release --lib -- --ignored timing_n_plus_one --nocapture --test-threads=1`

use crate::db::kg::KgFactInput;
use crate::db::Store;
use crate::models::{Chunk, Collection, CollectionEntry, Document};
use std::time::Instant;

fn median_ms(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn seed_documents(store: &Store, count: usize, with_chunks: bool) {
    for index in 0..count {
        let id = format!("n1-doc-{index}");
        store
            .documents()
            .upsert_document(&Document {
                id: id.clone(),
                uri: format!("n1://{index}"),
                title: format!("N+1 document {index}"),
                content: format!("body of document {index}"),
                metadata_json: "{}".into(),
                ..Default::default()
            })
            .unwrap();
        if with_chunks {
            store
                .chunks()
                .replace_chunks_for_document(
                    &id,
                    &[Chunk {
                        id: format!("{id}-c0"),
                        document_id: id.clone(),
                        chunk_index: 0,
                        content: format!("body of document {index}"),
                        embedding: vec![1.0, 0.0],
                        char_start: 0,
                        char_end: 10,
                        metadata_json: "{}".into(),
                    }],
                )
                .unwrap();
        }
    }
}

#[test]
#[ignore = "timing baseline; run explicitly"]
fn timing_n_plus_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("n1.duckdb")).unwrap();
    const DOCS: usize = 2_000;
    seed_documents(&store, DOCS, true);
    // Documents without chunks (non-blank bodies) for the health count.
    for index in 0..200 {
        store
            .documents()
            .upsert_document(&Document {
                id: format!("n1-bare-{index}"),
                uri: format!("n1-bare://{index}"),
                title: "bare".into(),
                content: "not chunked".into(),
                metadata_json: "{}".into(),
                ..Default::default()
            })
            .unwrap();
    }

    // Tags: 100 nodes x 20 labels, replace mode, labels shared across nodes.
    let labels: Vec<String> = (0..20).map(|i| format!("topic-{i}")).collect();
    let tags = median_ms(
        (0..3)
            .map(|round| {
                let started = Instant::now();
                for node in 0..100 {
                    store
                        .graph()
                        .set_document_tags(&format!("n1-node-{node}"), &labels, round % 2 == 0)
                        .unwrap();
                }
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    // Collections: 200 members, each depending on the previous one.
    let entries: Vec<CollectionEntry> = (0..200)
        .map(|index| CollectionEntry {
            document_id: format!("n1-doc-{index}"),
            position: index,
            parent_document_id: None,
            depends_on: if index == 0 {
                Vec::new()
            } else {
                vec![format!("n1-doc-{}", index - 1)]
            },
        })
        .collect();
    let mut create = Vec::new();
    let mut get = Vec::new();
    for round in 0..3 {
        let collection = Collection {
            id: format!("n1-collection-{round}"),
            name: "N+1".into(),
            description: None,
            metadata_json: "{}".into(),
            created_at: "2026-01-01 00:00:00".into(),
            updated_at: "2026-01-01 00:00:00".into(),
        };
        let started = Instant::now();
        store
            .collections()
            .create_collection(&collection, &entries)
            .unwrap();
        create.push(started.elapsed().as_secs_f64() * 1000.0);
        let started = Instant::now();
        let detail = store
            .collections()
            .get_collection(&collection.id)
            .unwrap()
            .unwrap();
        get.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(detail.entries.len(), 200);
    }

    // KG: 2 000 facts; stats, then invalidating 200 of them.
    for index in 0..2_000 {
        store
            .kg()
            .kg_add(KgFactInput {
                subject: &format!("subject-{}", index % 300),
                predicate: &format!("rel_{}", index % 7),
                object: &format!("object-{index}"),
                valid_from: None,
                valid_to: None,
                source_document_id: None,
                confidence: None,
                metadata_json: None,
            })
            .unwrap();
    }
    let stats = median_ms(
        (0..20)
            .map(|_| {
                let started = Instant::now();
                store.kg().kg_stats().unwrap();
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    let started = Instant::now();
    for index in 0..200 {
        let closed = store
            .kg()
            .kg_invalidate(
                &format!("subject-{}", index % 300),
                &format!("rel_{}", index % 7),
                &format!("object-{index}"),
                None,
            )
            .unwrap();
        assert_eq!(closed.len(), 1);
    }
    let invalidate = started.elapsed().as_secs_f64() * 1000.0;

    // Health: chunkless count over 2 200 documents (200 chunkless).
    let health = median_ms(
        (0..20)
            .map(|_| {
                let started = Instant::now();
                let counts = store.maintenance().integrity_counts().unwrap();
                assert_eq!(counts.0, 200);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    println!(
        "TIMING n_plus_one tags_100x20_ms={tags:.1} collection_create_200_ms={:.1} collection_get_200_ms={:.2} kg_stats_ms={stats:.2} kg_invalidate_200_ms={invalidate:.1} health_integrity_ms={health:.2}",
        median_ms(create),
        median_ms(get),
    );
}
