use super::*;
use crate::testkit::temp_store;

fn base(id: &str, content: &str) -> Document {
    Document {
        id: id.into(),
        uri: format!("wiki://{id}"),
        title: id.into(),
        content: content.into(),
        metadata_json: "{}".into(),
        ..Document::default()
    }
}

#[test]
fn insert_new_persists_a_pending_first_revision_with_sources() {
    let store = temp_store();
    let source = ReviewSource {
        document_id: "src".into(),
        uri: "text://src".into(),
        content_hash: Some(content_hash("source body")),
        content: Some("source body".into()),
    };
    let proposal = insert_new(&store, base("page", "draft body"), vec![source]).unwrap();
    assert!(uuid::Uuid::parse_str(&proposal.id).is_ok());
    assert_eq!(proposal.status, "pending");
    assert_eq!(proposal.revision, 1);
    // The editable content starts as a copy of the base document body.
    assert_eq!(proposal.content, "draft body");
    assert_eq!(proposal.base.id, "page");

    let stored = get(&store, &proposal.id).unwrap();
    assert_eq!(stored.content, "draft body");
    assert_eq!(stored.revision, 1);
    assert_eq!(stored.sources.len(), 1);
    assert_eq!(stored.sources[0].uri, "text://src");
    assert_eq!(
        stored.sources[0].content_hash.as_deref(),
        Some(content_hash("source body").as_str())
    );
}

#[test]
fn insert_new_assigns_distinct_ids_and_feeds_update() {
    let store = temp_store();
    let first = insert_new(&store, base("a", "one"), Vec::new()).unwrap();
    let second = insert_new(&store, base("a", "one"), Vec::new()).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(list(&store).unwrap().len(), 2);

    let updated = update(&store, &first.id, 1, "edited".into(), false).unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.content, "edited");
    // A stale revision is a conflict, not a silent overwrite.
    assert!(update(&store, &first.id, 1, "stale".into(), false).is_err());
    assert_eq!(get(&store, &second.id).unwrap().content, "one");
}
