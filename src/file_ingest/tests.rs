use super::*;

#[test]
fn is_supported_source_accepts_docs_and_code_case_insensitively() {
    for name in [
        "a.md",
        "b.MARKDOWN",
        "c.txt",
        "d.HTML",
        "e.pdf",
        "f.rs",
        "g.Py",
        "h.yaml",
    ] {
        assert!(is_supported_source(Path::new(name)), "{name}");
    }
    for name in ["noext", "image.png", "archive.tar.gz", "binary.exe", ".md"] {
        assert!(!is_supported_source(Path::new(name)), "{name}");
    }
}

#[test]
fn extract_file_tags_format_language_and_source_path() {
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("notes.md");
    std::fs::write(&md, "# Title\nbody").unwrap();
    let extracted = extract_file(&md).unwrap();
    assert_eq!(extracted.text, "# Title\nbody");
    assert_eq!(extracted.metadata["format"], "markdown");
    assert_eq!(
        extracted.metadata["source_path"],
        md.display().to_string().as_str()
    );
    assert!(!extracted.metadata.contains_key("language"));

    let rs = dir.path().join("lib.RS");
    std::fs::write(&rs, "fn main() {}").unwrap();
    let code = extract_file(&rs).unwrap();
    assert_eq!(code.metadata["format"], "source_code");
    assert_eq!(code.metadata["language"], "rust");

    let other = dir.path().join("README");
    std::fs::write(&other, "plain").unwrap();
    assert_eq!(extract_file(&other).unwrap().metadata["format"], "text");
}

#[test]
fn extract_file_strips_html_markup_scripts_and_entities() {
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("page.htm");
    std::fs::write(
        &html,
        "<html><head><style>p{color:red}</style><script>alert(1)</script></head>\
         <body><h1>Head</h1><p>A &amp; B&nbsp;&lt;tag&gt;</p><div>Two</div></body></html>",
    )
    .unwrap();
    let extracted = extract_file(&html).unwrap();
    assert_eq!(extracted.metadata["format"], "html");
    assert_eq!(extracted.text, "Head\nA & B <tag>\nTwo");
}

#[test]
fn extract_file_rejects_binary_invalid_utf8_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("blob.txt");
    std::fs::write(&binary, b"abc\0def").unwrap();
    assert!(matches!(
        extract_file(&binary),
        Err(FileIngestError::Binary(_))
    ));

    let latin1 = dir.path().join("latin1.txt");
    std::fs::write(&latin1, [0x63, 0x61, 0x66, 0xe9]).unwrap();
    assert!(matches!(
        extract_file(&latin1),
        Err(FileIngestError::Utf8 { .. })
    ));

    assert!(matches!(
        extract_file(&dir.path().join("missing.md")),
        Err(FileIngestError::Read { .. })
    ));
}

#[test]
fn merge_metadata_keeps_existing_keys_and_adds_new_ones() {
    let mut additions = Map::new();
    additions.insert("format".into(), Value::String("markdown".into()));
    additions.insert("source_path".into(), Value::String("/a.md".into()));

    let merged = merge_metadata(
        Some(r#"{"format":"custom","tag":1}"#.into()),
        additions.clone(),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&merged).unwrap();
    assert_eq!(value["format"], "custom", "caller metadata wins");
    assert_eq!(value["tag"], 1);
    assert_eq!(value["source_path"], "/a.md");

    for blank in [None, Some(String::new()), Some("   ".into())] {
        let value: Value =
            serde_json::from_str(&merge_metadata(blank, additions.clone()).unwrap()).unwrap();
        assert_eq!(value["format"], "markdown");
    }

    assert!(merge_metadata(Some("{bad".into()), Map::new()).is_err());
}
