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
    #[error("refusing to ingest binary-looking file {0}")]
    Binary(String),
    #[error("cannot extract PDF {path}: pdftotext was not found; install Poppler")]
    PdfToolMissing { path: String },
    #[error("cannot extract PDF {path}: pdftotext failed: {message}")]
    PdfExtract { path: String, message: String },
}

#[derive(Debug)]
pub struct ExtractedFile {
    pub text: String,
    pub metadata: Map<String, Value>,
}

pub fn is_supported_source(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "txt" | "html" | "htm" | "pdf"
        ) || source_language(ext).is_some())
}

pub fn extract_file(path: &Path) -> Result<ExtractedFile, FileIngestError> {
    let extension = path.extension().and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let format = match extension.as_deref() {
        Some("html" | "htm") => "html",
        Some("pdf") => "pdf",
        Some(ext) if source_language(ext).is_some() => "source_code",
        Some("md" | "markdown") => "markdown",
        _ => "text",
    };
    let text = if format == "pdf" {
        extract_pdf(path)?
    } else {
        let bytes = std::fs::read(path).map_err(|source| FileIngestError::Read {
            path: path.display().to_string(), source,
        })?;
        if bytes.contains(&0) {
            return Err(FileIngestError::Binary(path.display().to_string()));
        }
        let text = String::from_utf8(bytes).map_err(|source| FileIngestError::Utf8 {
            path: path.display().to_string(), source,
        })?;
        if format == "html" { html_to_text(&text) } else { text }
    };

    let mut metadata = Map::new();
    metadata.insert("format".into(), Value::String(format.into()));
    metadata.insert("source_path".into(), Value::String(path.display().to_string()));
    if let Some(language) = extension.as_deref().and_then(source_language) {
        metadata.insert("language".into(), Value::String(language.into()));
    }
    Ok(ExtractedFile { text, metadata })
}

pub fn merge_metadata(existing: Option<String>, additions: Map<String, Value>) -> Result<String, serde_json::Error> {
    let mut value = match existing.filter(|s| !s.trim().is_empty()) {
        Some(raw) => serde_json::from_str::<Value>(&raw)?,
        None => Value::Object(Map::new()),
    };
    if let Value::Object(object) = &mut value {
        for (key, addition) in additions {
            object.entry(key).or_insert(addition);
        }
    }
    Ok(value.to_string())
}

fn extract_pdf(path: &Path) -> Result<String, FileIngestError> {
    let output = Command::new("pdftotext").arg("-layout").arg(path).arg("-").output()
        .map_err(|error| if error.kind() == std::io::ErrorKind::NotFound {
            FileIngestError::PdfToolMissing { path: path.display().to_string() }
        } else {
            FileIngestError::Read { path: path.display().to_string(), source: error }
        })?;
    if !output.status.success() {
        return Err(FileIngestError::PdfExtract {
            path: path.display().to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    String::from_utf8(output.stdout).map_err(|source| FileIngestError::Utf8 {
        path: path.display().to_string(), source,
    })
}

fn source_language(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust", "py" => "python", "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript", "jsx" => "javascript", "java" => "java",
        "c" | "h" => "c", "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp", "go" => "go", "rb" => "ruby", "php" => "php",
        "swift" => "swift", "kt" | "kts" => "kotlin", "scala" => "scala",
        "sh" | "bash" | "zsh" => "shell", "sql" => "sql", "lua" => "lua",
        "ex" | "exs" => "elixir", "erl" | "hrl" => "erlang", "r" => "r",
        "vue" => "vue", "svelte" => "svelte", "toml" => "toml", "yaml" | "yml" => "yaml",
        "json" => "json", "xml" => "xml", "css" | "scss" | "sass" | "less" => "css",
        _ => return None,
    })
}

fn html_to_text(html: &str) -> String {
    let mut output = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut suppressed: Option<String> = None;
    for character in html.chars() {
        if in_tag {
            if character == '>' {
                let normalized = tag.trim().to_ascii_lowercase();
                let name = normalized.trim_start_matches('/').split_whitespace().next().unwrap_or("");
                if normalized.starts_with('/') && suppressed.as_deref() == Some(name) { suppressed = None; }
                else if !normalized.starts_with('/') && matches!(name, "script" | "style") { suppressed = Some(name.to_string()); }
                if suppressed.is_none() && matches!(name, "p" | "div" | "br" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr") {
                    output.push('\n');
                }
                tag.clear();
                in_tag = false;
            } else { tag.push(character); }
        } else if character == '<' { in_tag = true; }
        else if suppressed.is_none() { output.push(character); }
    }
    let decoded = output.replace("&nbsp;", " ").replace("&amp;", "&")
        .replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
    decoded.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n")
}
