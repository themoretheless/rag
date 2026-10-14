use super::*;
use crate::testkit::{temp_store, SeedDoc};

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

fn lifecycle_meta(store: &Store, id: &str) -> Value {
    let doc = store.get_document(id).unwrap().unwrap();
    doc.metadata().unwrap()["memory_lifecycle"].clone()
}

#[test]
fn list_candidates_filters_by_status_layer_and_kind() {
    let store = temp_store();
    SeedDoc::new("raw-a", "Raw A", "alpha").insert(&store);
    SeedDoc::new("raw-b", "Raw B", "beta")
        .kind("note")
        .insert(&store);
    SeedDoc::new("wiki-a", "Wiki A", "gamma")
        .layer("wiki")
        .insert(&store);
    archive_selected(&store, &ids(&["raw-b"]), None).unwrap();

    let mut active: Vec<String> = list_candidates(&store, None, None, None, 50)
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    active.sort();
    assert_eq!(active, ids(&["raw-a", "wiki-a"]));

    let raw = list_candidates(&store, None, Some("raw"), None, 50).unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].id, "raw-a");
    assert_eq!(raw[0].layer, "raw");
    assert_eq!(raw[0].status, STATUS_ACTIVE);

    let archived = list_candidates(&store, Some(STATUS_ARCHIVED), None, Some("note"), 50).unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, "raw-b");
    assert_eq!(archived[0].kind, "note");

    // A zero limit is clamped up to one row rather than returning nothing.
    assert_eq!(
        list_candidates(&store, None, None, None, 0).unwrap().len(),
        1
    );
}

#[test]
fn archive_selected_reports_changed_skipped_and_missing_and_is_idempotent() {
    let store = temp_store();
    SeedDoc::new("a", "A", "alpha").insert(&store);
    SeedDoc::new("b", "B", "beta").insert(&store);

    let first = archive_selected(
        &store,
        &ids(&["a", " a ", "", "ghost", "b"]),
        Some("tester"),
    )
    .unwrap();
    assert_eq!(first.operation, "archive");
    assert_eq!(first.requested, 5);
    assert_eq!(first.changed_ids, ids(&["a", "b"]));
    assert!(first.skipped_ids.is_empty());
    assert_eq!(first.missing_ids, ids(&["ghost"]));
    assert_eq!(first.output_document_id, None);
    assert_eq!(
        store.get_document("a").unwrap().unwrap().status,
        STATUS_ARCHIVED
    );

    let second = archive_selected(&store, &ids(&["a"]), None).unwrap();
    assert!(second.changed_ids.is_empty());
    assert_eq!(second.skipped_ids, ids(&["a"]));

    let logged = store
        .list_ops_log(50)
        .unwrap()
        .into_iter()
        .filter(|e| e.op == "memory_lifecycle_archive")
        .collect::<Vec<_>>();
    assert_eq!(logged.len(), 2);
    assert!(logged
        .iter()
        .any(|e| e.agent_name.as_deref() == Some("tester")));
}

#[test]
fn consolidate_selected_links_sources_to_output_and_skips_repeats() {
    let store = temp_store();
    SeedDoc::new("out", "Summary", "summary").insert(&store);
    SeedDoc::new("s1", "S1", "one")
        .uri("text://s1")
        .insert(&store);
    SeedDoc::new("s2", "S2", "two")
        .uri("text://s2")
        .insert(&store);

    let result = consolidate_selected(
        &store,
        &ids(&["s1", "s2", "out", "nope"]),
        " out ",
        Some("agent"),
    )
    .unwrap();
    assert_eq!(result.operation, "consolidate");
    assert_eq!(result.changed_ids, ids(&["s1", "s2"]));
    assert_eq!(result.missing_ids, ids(&["nope"]));
    assert_eq!(result.output_document_id.as_deref(), Some("out"));

    let output = lifecycle_meta(&store, "out");
    assert_eq!(output["consolidated_from"], json!(["s1", "s2"]));
    assert_eq!(output["source_uris"], json!(["text://s1", "text://s2"]));
    assert_eq!(output["operation"], "consolidate");
    assert_eq!(
        store.get_document("out").unwrap().unwrap().status,
        STATUS_ACTIVE
    );

    let s1 = store.get_document("s1").unwrap().unwrap();
    assert_eq!(s1.status, STATUS_CONSOLIDATED);
    assert_eq!(lifecycle_meta(&store, "s1")["consolidated_into"], "out");

    let again = consolidate_selected(&store, &ids(&["s1", "s2"]), "out", None).unwrap();
    assert!(again.changed_ids.is_empty());
    assert_eq!(again.skipped_ids, ids(&["s1", "s2"]));
    // Repeated merges do not duplicate provenance entries.
    assert_eq!(
        lifecycle_meta(&store, "out")["consolidated_from"],
        json!(["s1", "s2"])
    );
}

#[test]
fn consolidate_selected_rejects_blank_or_unknown_output() {
    let store = temp_store();
    SeedDoc::new("s1", "S1", "one").insert(&store);
    assert!(consolidate_selected(&store, &ids(&["s1"]), "  ", None).is_err());
    assert!(consolidate_selected(&store, &ids(&["s1"]), "missing", None).is_err());
    assert_eq!(
        store.get_document("s1").unwrap().unwrap().status,
        STATUS_ACTIVE
    );
}
