use super::*;

fn chunk_at(start: i32) -> (String, i32, i32) {
    (String::new(), start, start + 1)
}

fn offset_of(text: &str, needle: &str) -> i32 {
    let byte = text.find(needle).unwrap();
    text[..byte].chars().count() as i32
}

#[test]
fn markdown_sections_tracks_the_heading_path_at_each_chunk_start() {
    let text = "intro\n# Top\nbody\n## Sub\nmore\n### Leaf\nleaf body\n## Sibling\nend\n";
    let chunks = vec![
        chunk_at(0),
        chunk_at(offset_of(text, "# Top")),
        chunk_at(offset_of(text, "body")),
        chunk_at(offset_of(text, "leaf body")),
        chunk_at(offset_of(text, "end")),
    ];
    let sections = markdown_sections(text, &chunks);
    assert_eq!(sections.len(), chunks.len());
    assert_eq!(sections[0], (Vec::<String>::new(), None));
    // A heading that starts exactly at the chunk offset is already active.
    assert_eq!(sections[1], (vec!["Top".to_string()], Some("Top".into())));
    assert_eq!(sections[2], (vec!["Top".to_string()], Some("Top".into())));
    assert_eq!(
        sections[3],
        (
            vec!["Top".to_string(), "Sub".into(), "Leaf".into()],
            Some("Leaf".into())
        )
    );
    // A same-level heading replaces the previous sibling and drops its children.
    assert_eq!(
        sections[4],
        (
            vec!["Top".to_string(), "Sibling".into()],
            Some("Sibling".into())
        )
    );
}

#[test]
fn markdown_sections_ignores_non_headings_and_skipped_levels_leave_no_gaps() {
    let text = "#hashtag\n####### seven\n#   \n### Deep ###\nx\n";
    let sections = markdown_sections(text, &[chunk_at(offset_of(text, "x"))]);
    assert_eq!(
        sections,
        vec![(vec!["Deep".to_string()], Some("Deep".to_string()))]
    );
}

#[test]
fn markdown_sections_counts_offsets_in_chars_not_bytes() {
    let text = "ééé\n# Ünïcode\nbody\n";
    let heading = offset_of(text, "# Ünïcode");
    assert_eq!(heading, 4);
    let sections = markdown_sections(text, &[chunk_at(heading - 1), chunk_at(heading)]);
    assert_eq!(sections[0].1, None);
    assert_eq!(sections[1].1.as_deref(), Some("Ünïcode"));
}

#[test]
fn markdown_section_metadata_encodes_sections_as_json() {
    let text = "# A\nbody\n";
    let metadata = markdown_section_metadata(text, &[chunk_at(0)]);
    let value: serde_json::Value = serde_json::from_str(&metadata[0]).unwrap();
    assert_eq!(value["section"], "A");
    assert_eq!(value["heading_path"], serde_json::json!(["A"]));
    assert_eq!(
        markdown_section_metadata("plain", &[chunk_at(0)]),
        vec!["{}"]
    );
}
