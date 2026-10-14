use super::*;
use crate::testkit::{temp_store, SeedDoc};

fn collection(id: &str, name: &str) -> Collection {
    let now = chrono::Utc::now().to_rfc3339();
    Collection {
        id: id.into(),
        name: name.into(),
        description: Some("desc".into()),
        metadata_json: r#"{"k":1}"#.into(),
        created_at: now.clone(),
        updated_at: now,
    }
}

fn entry(id: &str, parent: Option<&str>, deps: &[&str]) -> CollectionEntry {
    CollectionEntry {
        document_id: id.into(),
        position: 0,
        parent_document_id: parent.map(str::to_string),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
    }
}

fn seeded_store() -> Store {
    let store = temp_store();
    for id in ["a", "b", "c"] {
        SeedDoc::new(id, id, "body").insert(&store);
    }
    store
}

#[test]
fn create_collection_persists_outline_order_and_dependency_order() {
    let store = seeded_store();
    let detail = store
        .create_collection(
            &collection("col-1", "Course"),
            &[
                entry("c", None, &["b"]),
                entry("a", None, &[]),
                entry("b", Some("a"), &["a"]),
            ],
        )
        .unwrap();
    assert_eq!(detail.collection.id, "col-1");
    assert_eq!(detail.collection.name, "Course");
    assert_eq!(detail.collection.description.as_deref(), Some("desc"));
    let order: Vec<(&str, i32)> = detail
        .entries
        .iter()
        .map(|e| (e.document_id.as_str(), e.position))
        .collect();
    // Positions come from the input order, not from caller-supplied values.
    assert_eq!(order, vec![("c", 0), ("a", 1), ("b", 2)]);
    assert_eq!(detail.entries[2].parent_document_id.as_deref(), Some("a"));
    assert_eq!(detail.entries[0].depends_on, vec!["b".to_string()]);
    assert_eq!(detail.dependency_order, vec!["a", "b", "c"]);
    assert!(detail.dependency_cycle_members.is_empty());
}

#[test]
fn create_collection_rejects_invalid_input_without_writing() {
    let store = seeded_store();
    let bad = [
        (collection("", "Name"), vec![]),
        (collection("x", "  "), vec![]),
        (
            Collection {
                metadata_json: "not json".into(),
                ..collection("x", "Name")
            },
            vec![],
        ),
        (collection("x", "Name"), vec![entry("missing", None, &[])]),
        (
            collection("x", "Name"),
            vec![entry("a", None, &[]), entry("a", None, &[])],
        ),
        (collection("x", "Name"), vec![entry("a", Some("b"), &[])]),
        (collection("x", "Name"), vec![entry("a", None, &["a"])]),
        (
            collection("x", "Name"),
            vec![entry("a", Some("b"), &[]), entry("b", Some("a"), &[])],
        ),
    ];
    for (col, entries) in bad {
        assert!(
            store.create_collection(&col, &entries).is_err(),
            "accepted {col:?} {entries:?}"
        );
    }
    assert!(store.list_collections().unwrap().is_empty());
    assert!(store.get_collection("x").unwrap().is_none());
}

#[test]
fn dependency_cycles_are_reported_instead_of_ordered() {
    let store = seeded_store();
    let detail = store
        .create_collection(
            &collection("cyc", "Cycle"),
            &[
                entry("a", None, &["b"]),
                entry("b", None, &["a"]),
                entry("c", None, &[]),
            ],
        )
        .unwrap();
    assert!(detail.dependency_order.is_empty());
    assert_eq!(detail.dependency_cycle_members, vec!["a", "b"]);
}

#[test]
fn list_and_get_collections() {
    let store = seeded_store();
    assert!(store.get_collection("nope").unwrap().is_none());
    store
        .create_collection(&collection("one", "One"), &[entry("a", None, &[])])
        .unwrap();
    store
        .create_collection(&collection("two", "Two"), &[])
        .unwrap();
    let mut names: Vec<String> = store
        .list_collections()
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["One", "Two"]);
    let one = store.get_collection("one").unwrap().unwrap();
    assert_eq!(one.entries.len(), 1);
    assert_eq!(one.collection.metadata_json, r#"{"k":1}"#);
    assert!(store
        .get_collection("two")
        .unwrap()
        .unwrap()
        .entries
        .is_empty());
}

#[test]
fn update_collection_patches_fields_and_replaces_entries() {
    let store = seeded_store();
    store
        .create_collection(
            &collection("col", "Old"),
            &[entry("a", None, &[]), entry("b", None, &["a"])],
        )
        .unwrap();

    // Only the name changes; description, metadata and entries are kept.
    let renamed = store
        .update_collection("col", Some("  New  "), None, None, None)
        .unwrap();
    assert_eq!(renamed.collection.name, "New");
    assert_eq!(renamed.collection.description.as_deref(), Some("desc"));
    assert_eq!(renamed.entries.len(), 2);

    let cleared = store
        .update_collection(
            "col",
            None,
            Some(None),
            Some(r#"{"v":2}"#),
            Some(&[entry("c", None, &[])]),
        )
        .unwrap();
    assert_eq!(cleared.collection.description, None);
    assert_eq!(cleared.collection.metadata_json, r#"{"v":2}"#);
    let ids: Vec<&str> = cleared
        .entries
        .iter()
        .map(|e| e.document_id.as_str())
        .collect();
    assert_eq!(ids, vec!["c"]);

    assert!(store
        .update_collection("col", Some(" "), None, None, None)
        .is_err());
    assert!(store
        .update_collection("col", None, None, Some("{"), None)
        .is_err());
    assert!(store
        .update_collection("col", None, None, None, Some(&[entry("zzz", None, &[])]))
        .is_err());
    assert!(store
        .update_collection("missing", Some("X"), None, None, None)
        .is_err());
    let unchanged = store.get_collection("col").unwrap().unwrap();
    assert_eq!(unchanged.collection.name, "New");
    assert_eq!(unchanged.entries.len(), 1);
}
