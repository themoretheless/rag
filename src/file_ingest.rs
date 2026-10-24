//! Safe, format-aware file extraction for the ingest entry points.

use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileIngestError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid UTF-8 text: {source}")]
    Utf8 {
        path: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("cannot decode {path}: {message}")]
    Encoding { path: String, message: String },
    #[error("refusing to ingest binary-looking file {0}")]
    Binary(String),
    #[error("cannot extract PDF {path}: pdftotext was not found; install Poppler")]
    PdfToolMissing { path: String },
    #[error("cannot extract PDF {path}: pdftotext failed: {message}")]
    PdfExtract { path: String, message: String },
    #[error("cannot extract {format} {path}: {message}")]
    Office {
        path: String,
        format: &'static str,
        message: String,
    },
}

#[derive(Debug)]
pub struct ExtractedFile {
    pub text: String,
    pub metadata: Map<String, Value>,
}

pub fn is_supported_source(path: &Path) -> bool {
    if path.file_name().is_some_and(|n| n == "go.mod") {
        return true;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "txt" | "html" | "htm" | "pdf" | "docx" | "xlsx"
            ) || source_language(ext).is_some()
        })
}

/// Deterministic byte decoding; legacy choices are explicitly heuristic.
pub(crate) fn decode_text(
    bytes: &[u8],
    path: &Path,
) -> Result<(String, &'static str, &'static str), FileIngestError> {
    use encoding_rs::{UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1251, WINDOWS_1252};
    let fail = || FileIngestError::Encoding {
        path: path.display().to_string(),
        message: "malformed declared encoding; replacement characters are not inserted".into(),
    };
    let (text, encoding, basis) = if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        (
            UTF_8
                .decode_without_bom_handling_and_without_replacement(bytes)
                .ok_or_else(fail)?
                .into_owned(),
            "utf-8-sig",
            "bom",
        )
    } else if bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
        return Err(FileIngestError::Encoding {
            path: path.display().to_string(),
            message: "UTF-32 is unsupported".into(),
        });
    } else if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let little = bytes.starts_with(&[0xff, 0xfe]);
        let decoder = if little { UTF_16LE } else { UTF_16BE };
        (
            decoder
                .decode_without_bom_handling_and_without_replacement(&bytes[2..])
                .ok_or_else(fail)?
                .into_owned(),
            if little { "utf-16le" } else { "utf-16be" },
            "bom",
        )
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        (text.into(), "utf-8", "strict_utf8")
    } else {
        // Single-byte Cyrillic evidence, matching the reference's deterministic
        // threshold. This is not proof of the author's intended encoding.
        let cyr = bytes
            .iter()
            .filter(|b| matches!(**b, 0xc0..=0xff | 0xa8 | 0xb8))
            .count();
        let letters = cyr + bytes.iter().filter(|b| b.is_ascii_alphabetic()).count();
        let russian = cyr >= 4 && cyr.saturating_mul(20) >= letters;
        let decoder = if russian { WINDOWS_1251 } else { WINDOWS_1252 };
        let text = decoder
            .decode_without_bom_handling_and_without_replacement(bytes)
            .ok_or_else(fail)?
            .into_owned();
        (
            text,
            if russian {
                "windows-1251"
            } else {
                "windows-1252"
            },
            if russian {
                "heuristic_cyrillic_share"
            } else {
                "heuristic_western_fallback"
            },
        )
    };
    if text.contains('\0')
        || (basis.starts_with("heuristic")
            && text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}')))
    {
        return Err(FileIngestError::Binary(path.display().to_string()));
    }
    Ok((text, encoding, basis))
}

pub(crate) fn decoding_metadata_current(path: &Path, metadata: Option<&str>) -> bool {
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "pdf" | "docx" | "xlsx"))
    {
        return true;
    }
    metadata
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .is_some_and(|v| {
            v["source_decoding_version"] == 1
                && v["source_text_hash"].is_string()
                && v["source_bytes_hash"].is_string()
        })
}

pub fn extract_file(path: &Path) -> Result<ExtractedFile, FileIngestError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let format = match extension.as_deref() {
        _ if path.file_name().is_some_and(|n| n == "go.mod") => "source_code",
        Some("html" | "htm") => "html",
        Some("pdf") => "pdf",
        Some("docx") => "docx",
        Some("xlsx") => "xlsx",
        Some(ext) if source_language(ext).is_some() => "source_code",
        Some("md" | "markdown") => "markdown",
        _ => "text",
    };
    let mut original_html: Option<String> = None;
    let mut decoding = Map::new();
    let mut warnings: Vec<String> = Vec::new();
    let text = match format {
        "pdf" => {
            let (text, warning) = extract_pdf(path)?;
            if let Some(warning) = warning {
                tracing::warn!(path = %path.display(), "{warning}");
                warnings.push(warning);
            }
            text
        }
        "docx" => {
            let (text, heading_count) = crate::ooxml::extract_docx(path)?;
            tracing::debug!(path = %path.display(), heading_count, "extracted docx");
            text
        }
        "xlsx" => crate::ooxml::extract_xlsx(path)?,
        _ => {
            let bytes = std::fs::read(path).map_err(|source| FileIngestError::Read {
                path: path.display().to_string(),
                source,
            })?;
            let (text, encoding, basis) = decode_text(&bytes, path)?;
            decoding.insert("source_encoding".into(), serde_json::json!(encoding));
            decoding.insert("source_encoding_basis".into(), serde_json::json!(basis));
            decoding.insert(
                "source_bytes_hash".into(),
                serde_json::json!(blake3::hash(&bytes).to_hex().to_string()),
            );
            decoding.insert(
                "source_text_hash".into(),
                serde_json::json!(crate::util::content_hash(&text)),
            );
            decoding.insert("source_decoding_version".into(), serde_json::json!(1));
            if format == "html" {
                let readable = html_to_text(&text);
                original_html = Some(text);
                readable
            } else {
                text
            }
        }
    };

    let mut metadata = decoding;
    if let Some(original) = original_html {
        metadata.insert(
            "code_source_hash".into(),
            serde_json::json!(crate::util::content_hash(&original)),
        );
        metadata.insert(
            "code_source_kind".into(),
            serde_json::json!("original_html"),
        );
        metadata.insert("code_source_text".into(), serde_json::json!(original));
    }
    metadata.insert("format".into(), Value::String(format.into()));
    metadata.insert(
        "source_path".into(),
        Value::String(path.display().to_string()),
    );
    if !warnings.is_empty() {
        metadata.insert(
            "extract_warnings".into(),
            Value::Array(warnings.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(language) = extension.as_deref().and_then(source_language) {
        metadata.insert("language".into(), Value::String(language.into()));
    }
    Ok(ExtractedFile { text, metadata })
}

pub fn merge_metadata(
    existing: Option<String>,
    additions: Map<String, Value>,
) -> Result<String, serde_json::Error> {
    let mut value = match existing.filter(|s| !s.trim().is_empty()) {
        Some(raw) => serde_json::from_str::<Value>(&raw)?,
        None => Value::Object(Map::new()),
    };
    if let Value::Object(object) = &mut value {
        for (key, addition) in additions {
            if matches!(
                key.as_str(),
                "code_source_hash"
                    | "code_source_kind"
                    | "code_source_text"
                    | "source_encoding"
                    | "source_encoding_basis"
                    | "source_bytes_hash"
                    | "source_text_hash"
                    | "source_decoding_version"
            ) {
                object.insert(key, addition);
            } else {
                object.entry(key).or_insert(addition);
            }
        }
    }
    Ok(value.to_string())
}

/// Run Poppler `pdftotext`. Returns the extracted text plus an optional
/// warning when the output looks empty (scanned/image-only PDF) or garbled.
/// A pure-Rust library extractor was evaluated and rejected: `lopdf`-level
/// text assembly is not production-reasonable and `pdfium-render` needs a
/// native library, so the external-binary path stays and is hardened here.
fn extract_pdf(path: &Path) -> Result<(String, Option<String>), FileIngestError> {
    let text = run_pdftotext(path, true)?;
    // `-layout` preserves columns but occasionally yields nothing where the
    // plain reading-order mode succeeds; retry once before warning.
    let (text, used_layout) = if text.trim().is_empty() {
        (run_pdftotext(path, false)?, false)
    } else {
        (text, true)
    };
    let _ = used_layout;
    let warning = pdf_quality_warning(&text);
    Ok((text, warning))
}

fn run_pdftotext(path: &Path, layout: bool) -> Result<String, FileIngestError> {
    let mut command = Command::new("pdftotext");
    if layout {
        command.arg("-layout");
    }
    let output = command.arg(path).arg("-").output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FileIngestError::PdfToolMissing {
                path: path.display().to_string(),
            }
        } else {
            FileIngestError::Read {
                path: path.display().to_string(),
                source: error,
            }
        }
    })?;
    if !output.status.success() {
        return Err(FileIngestError::PdfExtract {
            path: path.display().to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    String::from_utf8(output.stdout).map_err(|source| FileIngestError::Utf8 {
        path: path.display().to_string(),
        source,
    })
}

/// Heuristic scan/garble detector. Scanned PDFs yield no (or only page-number)
/// text; broken encodings yield mostly non-alphanumeric glyphs.
fn pdf_quality_warning(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(
            "pdf produced no extractable text; it is likely scanned or image-only (OCR is not supported)".into(),
        );
    }
    let non_ws: Vec<char> = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if non_ws.is_empty() {
        return Some(
            "pdf produced no extractable text; it is likely scanned or image-only (OCR is not supported)".into(),
        );
    }
    // Very short output from a multi-form-feed document suggests image pages.
    let pages = trimmed.matches('\u{000C}').count() + 1;
    if pages >= 3 && non_ws.len() / pages < 20 {
        return Some(format!(
            "pdf yielded only {} characters across ~{pages} pages; it is likely mostly scanned images (OCR is not supported)",
            non_ws.len(),
        ));
    }
    let alnum = non_ws.iter().filter(|c| c.is_alphanumeric()).count();
    if alnum * 2 < non_ws.len() {
        return Some(format!(
            "pdf text looks garbled ({}% alphanumeric characters); extraction quality is low",
            alnum * 100 / non_ws.len(),
        ));
    }
    None
}

fn source_language(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "jsx" => "javascript",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "pl" | "pm" => "perl",
        "ps1" | "psm1" | "psd1" => "powershell",
        "phtml" => "php",
        "sh" | "bash" | "zsh" => "shell",
        "sql" => "sql",
        "lua" => "lua",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "r" => "r",
        "vue" => "vue",
        "svelte" => "svelte",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "xml" => "xml",
        "css" | "scss" | "sass" | "less" => "css",
        _ => return None,
    })
}

/// Readability-lite HTML extraction: drop boilerplate elements, prefer the
/// `article`/`main` subtree when present, keep headings as Markdown `#` lines,
/// list items as `- ` lines, and table rows as `cell | cell` lines.
///
/// Shared by file ingest (`*.html`) and web ingest (`ingest_url`).
pub fn html_to_text(html: &str) -> String {
    let document = dom_query::Document::from(html);
    document
        .select("script, style, noscript, nav, footer, header, aside, form, iframe, svg")
        .remove();
    let root = {
        let article = document.select_single("article");
        let main = document.select_single("main");
        let body = document.select_single("body");
        if article.exists() {
            article
        } else if main.exists() {
            main
        } else {
            body
        }
    };
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for node in root.nodes() {
        render_html_node(node, &mut lines, &mut current);
    }
    flush_html_line(&mut lines, &mut current);
    lines.join("\n")
}

fn flush_html_line(lines: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        lines.push(trimmed.to_string());
    }
    current.clear();
}

fn append_html_text(current: &mut String, text: &str) {
    let piece = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if piece.is_empty() {
        return;
    }
    // Avoid "word ." artifacts when punctuation is its own text node.
    let starts_with_punctuation = piece
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '%'));
    if !current.is_empty() && !starts_with_punctuation {
        current.push(' ');
    }
    current.push_str(&piece);
}

fn render_html_node(node: &dom_query::NodeRef<'_>, lines: &mut Vec<String>, current: &mut String) {
    if node.is_text() {
        append_html_text(current, &node.text());
        return;
    }
    let Some(name) = node.node_name() else { return };
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            flush_html_line(lines, current);
            let level = name[1..].parse::<usize>().unwrap_or(1);
            let text = node.text().split_whitespace().collect::<Vec<_>>().join(" ");
            if !text.is_empty() {
                lines.push(format!("{} {}", "#".repeat(level), text));
            }
        }
        "tr" => {
            flush_html_line(lines, current);
            let cells: Vec<String> = node
                .element_children()
                .into_iter()
                .filter(|child| {
                    child
                        .node_name()
                        .is_some_and(|n| matches!(n.to_ascii_lowercase().as_str(), "td" | "th"))
                })
                .map(|cell| cell.text().split_whitespace().collect::<Vec<_>>().join(" "))
                .collect();
            if cells.iter().any(|cell| !cell.is_empty()) {
                lines.push(cells.join(" | "));
            }
        }
        "li" => {
            flush_html_line(lines, current);
            current.push('-');
            for child in node.children() {
                render_html_node(&child, lines, current);
            }
            flush_html_line(lines, current);
        }
        "br" => flush_html_line(lines, current),
        // Block-level containers: hard line boundary before and after.
        "p" | "div" | "section" | "article" | "main" | "ul" | "ol" | "table" | "thead"
        | "tbody" | "blockquote" | "pre" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "hr" => {
            flush_html_line(lines, current);
            for child in node.children() {
                render_html_node(&child, lines, current);
            }
            flush_html_line(lines, current);
        }
        // Inline and unknown elements: recurse so text flows into the current line.
        _ => {
            for child in node.children() {
                render_html_node(&child, lines, current);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
