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
    assert_eq!(extracted.text, "# Head\nA & B <tag>\nTwo");
}

#[test]
fn extract_file_rejects_binary_and_missing_files_and_decodes_legacy_text() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("blob.txt");
    std::fs::write(&binary, b"abc\0def").unwrap();
    assert!(matches!(
        extract_file(&binary),
        Err(FileIngestError::Binary(_))
    ));

    let latin1 = dir.path().join("latin1.txt");
    std::fs::write(&latin1, [0x63, 0x61, 0x66, 0xe9]).unwrap();
    let legacy = extract_file(&latin1).unwrap();
    assert_eq!(legacy.text, "café");
    assert_eq!(legacy.metadata["source_encoding"], "windows-1252");
    assert_eq!(
        legacy.metadata["source_encoding_basis"],
        "heuristic_western_fallback"
    );

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

#[test]
fn text_decoding_boms_legacy_heuristics_and_invalid_input() {
    let path = Path::new("source.txt");
    for (bytes, text, encoding, basis) in [
        (
            "Привет".as_bytes().to_vec(),
            "Привет",
            "utf-8",
            "strict_utf8",
        ),
        (
            [vec![0xef, 0xbb, 0xbf], "Привет".as_bytes().to_vec()].concat(),
            "Привет",
            "utf-8-sig",
            "bom",
        ),
        (
            [
                vec![0xff, 0xfe],
                "Привет".encode_utf16().flat_map(u16::to_le_bytes).collect(),
            ]
            .concat(),
            "Привет",
            "utf-16le",
            "bom",
        ),
        (
            [
                vec![0xfe, 0xff],
                "Привет".encode_utf16().flat_map(u16::to_be_bytes).collect(),
            ]
            .concat(),
            "Привет",
            "utf-16be",
            "bom",
        ),
        (
            vec![0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2],
            "Привет",
            "windows-1251",
            "heuristic_cyrillic_share",
        ),
        (
            vec![b'c', b'a', b'f', 0xe9],
            "café",
            "windows-1252",
            "heuristic_western_fallback",
        ),
    ] {
        let result = decode_text(&bytes, path).unwrap();
        assert_eq!(result, (text.into(), encoding, basis));
    }
    for bytes in [
        vec![0xff, 0xfe, 0],
        vec![0xff, 0xfe, 0, 0xd8],
        vec![0xef, 0xbb, 0xbf, 0xff],
        vec![0xff, 0xfe, 0, 0],
        vec![0, 1, 2],
        vec![0xff, 1, 2, 3],
    ] {
        assert!(decode_text(&bytes, path).is_err(), "{bytes:?}");
    }
}

#[test]
fn extraction_refreshes_encoding_metadata_and_preserves_html_source() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("page.html");
    let html = "<p>Привет</p>";
    let (bytes, _, errors) = encoding_rs::WINDOWS_1251.encode(html);
    assert!(!errors);
    std::fs::write(&path, &bytes).unwrap();
    let old = extract_file(&path).unwrap();
    assert!(old.text.contains("Привет"));
    assert_eq!(old.metadata["source_encoding"], "windows-1251");
    assert_eq!(old.metadata["code_source_text"], html);
    std::fs::write(&path, html).unwrap();
    let new = extract_file(&path).unwrap();
    let merged = merge_metadata(
        Some(serde_json::to_string(&old.metadata).unwrap()),
        new.metadata,
    )
    .unwrap();
    let metadata: Value = serde_json::from_str(&merged).unwrap();
    assert_eq!(metadata["source_encoding"], "utf-8");
    assert_ne!(
        metadata["source_bytes_hash"],
        old.metadata["source_bytes_hash"]
    );
    assert!(decoding_metadata_current(&path, Some(&merged)));
    assert!(!decoding_metadata_current(&path, Some("{}")));
}

#[test]
fn html_extract_drops_boilerplate_and_prefers_article() {
    let html = r#"<!DOCTYPE html><html><head><title>Site</title>
            <style>body { color: red; }</style></head><body>
            <header><p>Big site header</p></header>
            <nav><a href="/">Home</a></nav>
            <article><h1>Real Title</h1><p>Main story text with <b>inline bold</b>.</p>
            <ul><li>first point</li><li>second point</li></ul></article>
            <aside><p>Sidebar junk</p></aside>
            <footer>Copyright 2026</footer>
            <script>alert('x')</script>
            </body></html>"#;
    let text = html_to_text(html);
    assert!(text.contains("# Real Title"), "heading: {text}");
    assert!(
        text.contains("Main story text with inline bold."),
        "paragraph: {text}"
    );
    assert!(text.contains("- first point"), "list item: {text}");
    assert!(!text.contains("Sidebar junk"), "aside dropped: {text}");
    assert!(!text.contains("Big site header"), "header dropped: {text}");
    assert!(!text.contains("Home"), "nav dropped: {text}");
    assert!(!text.contains("alert"), "script dropped: {text}");
    assert!(!text.contains("color: red"), "style dropped: {text}");
}

#[test]
fn html_extract_renders_tables_and_entities() {
    let html = r#"<html><body><main>
            <h2>Results</h2>
            <table><tr><th>Name</th><th>Score</th></tr>
            <tr><td>Ada &amp; Bob</td><td>9</td></tr></table>
            </main></body></html>"#;
    let text = html_to_text(html);
    assert!(text.contains("## Results"), "heading: {text}");
    assert!(text.contains("Name | Score"), "header row: {text}");
    assert!(
        text.contains("Ada & Bob | 9"),
        "data row with entity: {text}"
    );
}

#[test]
fn html_extract_handles_fragment_without_body() {
    let text = html_to_text("<p>Just a paragraph</p>");
    assert_eq!(text, "Just a paragraph");
}

#[test]
fn pdf_quality_warning_flags_empty_and_garbled() {
    assert!(pdf_quality_warning("   \n  ")
        .unwrap()
        .contains("no extractable text"));
    let garbled = "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}ab";
    assert!(pdf_quality_warning(garbled).unwrap().contains("garbled"));
    assert!(pdf_quality_warning("A normal sentence with mostly letters and words.").is_none());
}

#[test]
fn pdf_quality_warning_flags_sparse_multipage_output() {
    // ~1 char of text per page across 5 pages: image-only pages.
    let text = "1\u{000C}2\u{000C}3\u{000C}4\u{000C}5";
    assert!(pdf_quality_warning(text).unwrap().contains("scanned"));
}

#[test]
fn supported_extensions_include_office_formats() {
    assert!(is_supported_source(Path::new("notes.docx")));
    assert!(is_supported_source(Path::new("book.xlsx")));
    assert!(is_supported_source(Path::new("page.html")));
    assert!(!is_supported_source(Path::new("slides.pptx")));
}
