//! Safe, format-aware file extraction for the ingest entry points.

mod capture;
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

/// Whether the extension is one `extract_file` can read (Markdown, text,
/// HTML, PDF or a known source-code language).
pub fn is_supported_source(path: &Path) -> bool {
    if path.file_name().is_some_and(|n| {
        n == "go.mod"
            || n == "Dockerfile"
            || n == "Containerfile"
            || n.to_string_lossy().starts_with("Dockerfile.")
            || n.to_string_lossy().starts_with("Containerfile.")
    }) {
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

/// Read a file and extract its text and format-specific metadata.
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
    let snapshot = if matches!(format, "pdf" | "docx" | "xlsx") {
        Some(capture::Snapshot::read(path)?)
    } else {
        None
    };
    let read_path = snapshot.as_ref().map_or(path, |s| s.path());
    let mut original_html: Option<String> = None;
    let mut decoding = Map::new();
    let mut warnings: Vec<String> = Vec::new();
    let text = match format {
        "pdf" => {
            let (text, warning) =
                extract_pdf(read_path).map_err(|e| capture::source_error(e, path))?;
            if let Some(warning) = warning {
                tracing::warn!(path = %path.display(), "{warning}");
                warnings.push(warning);
            }
            text
        }
        "docx" => {
            let (text, heading_count) = crate::ooxml::extract_docx(read_path)
                .map_err(|e| capture::source_error(e, path))?;
            tracing::debug!(path = %path.display(), heading_count, "extracted docx");
            text
        }
        "xlsx" => {
            crate::ooxml::extract_xlsx(read_path).map_err(|e| capture::source_error(e, path))?
        }
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
            decoding.insert("source_bytes_count".into(), serde_json::json!(bytes.len()));
            if format == "html" {
                let readable = html_to_text(&text);
                original_html = Some(text);
                readable
            } else {
                text
            }
        }
    };

    if let Some(snapshot) = &snapshot {
        decoding.insert("source_bytes_hash".into(), serde_json::json!(snapshot.hash));
        decoding.insert(
            "source_bytes_count".into(),
            serde_json::json!(snapshot.bytes),
        );
    }
    decoding.insert("source_capture_version".into(), serde_json::json!(1));
    decoding.insert(
        "source_bytes_hash_algorithm".into(),
        serde_json::json!("blake3"),
    );
    decoding.insert(
        "source_text_hash".into(),
        serde_json::json!(crate::util::content_hash(
            original_html.as_deref().unwrap_or(&text)
        )),
    );
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

/// Merge `additions` into existing metadata JSON; keys already present win.
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
                    | "source_capture_version"
                    | "source_bytes_hash_algorithm"
                    | "source_bytes_count"
            ) {
                object.insert(key, addition);
            } else {
                object.entry(key).or_insert(addition);
            }
        }
    }
    Ok(value.to_string())
}

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
        "pl" | "pm" | "t" => "perl",
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
        "dockerfile" => "dockerfile",
        "json" => "json",
        "xml" | "svg" => "xml",
        "css" | "scss" | "sass" | "less" => "css",
        _ => return None,
    })
}

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

#[cfg(test)]
mod tests;

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

pub(crate) fn decoding_metadata_current(
    path: &Path,
    metadata: Option<&str>,
    stored_text_hash: Option<&str>,
) -> bool {
    let binary_document = path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "pdf" | "docx" | "xlsx"));
    metadata
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .is_some_and(|v| {
            let html = path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "html" | "htm"));
            let expected = if html {
                v["code_source_hash"].as_str()
            } else {
                stored_text_hash
            };
            expected.is_some()
                && v["source_text_hash"].as_str() == expected
                && v["source_capture_version"] == 1
                && v["source_bytes_hash_algorithm"] == "blake3"
                && v["source_bytes_count"].as_u64().is_some()
                && v["source_text_hash"].is_string()
                && v["source_bytes_hash"].as_str().is_some_and(|h| {
                    h.len() == 64
                        && h.bytes()
                            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                })
                && (binary_document || v["source_decoding_version"] == 1)
        })
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
