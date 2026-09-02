//! Markdown vault storage adapter.
//!
//! Each document is a Markdown file whose YAML 1.2 frontmatter is represented
//! by a JSON mapping (JSON is a valid YAML 1.2 subset). The files, rather than
//! a sidecar database, are the source of truth.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AppError, Document};

use super::{BackendKind, BackendMetadata, Storage, StorageCapability};

pub(super) const CAPABILITIES: &[StorageCapability] = &[StorageCapability::Documents];

#[derive(Debug)]
pub struct MarkdownVaultStorage {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct Frontmatter {
    id: String,
    uri: String,
    title: String,
    metadata_json: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    wing: Option<String>,
    room: Option<String>,
    source_file: Option<String>,
    layer: String,
    kind: String,
    content_hash: Option<String>,
    status: String,
    pinned: bool,
    boost: f64,
    revision: i64,
}

impl MarkdownVaultStorage {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, AppError> {
        let requested = root.as_ref();
        if requested.as_os_str().is_empty() {
            return Err(AppError::config("markdown vault root must not be empty"));
        }
        fs::create_dir_all(requested)?;
        let root = fs::canonicalize(requested)?;
        if !root.is_dir() {
            return Err(AppError::config(format!(
                "markdown vault root '{}' is not a directory",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn document_path(&self, document: &Document) -> PathBuf {
        self.root
            .join(encode_component(&document.layer))
            .join(format!("{}.md", encode_component(&document.id)))
    }

    fn markdown_files(&self) -> Result<Vec<PathBuf>, AppError> {
        let mut pending = vec![self.root.clone()];
        let mut files = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_symlink() {
                    continue;
                }
                if file_type.is_dir() {
                    if entry.file_name() != OsStr::new(".rag") {
                        pending.push(entry.path());
                    }
                } else if file_type.is_file()
                    && entry.path().extension() == Some(OsStr::new("md"))
                {
                    files.push(entry.path());
                }
            }
        }
        files.sort();
        Ok(files)
    }

    fn read_document(path: &Path) -> Result<Document, AppError> {
        let text = fs::read_to_string(path)?;
        let rest = text.strip_prefix("---\n").ok_or_else(|| {
            AppError::db(format!("'{}' has no YAML frontmatter", path.display()))
        })?;
        let (frontmatter, content) = rest.split_once("\n---\n").ok_or_else(|| {
            AppError::db(format!("'{}' has unterminated YAML frontmatter", path.display()))
        })?;
        let frontmatter: Frontmatter = serde_json::from_str(frontmatter).map_err(|error| {
            AppError::db(format!("invalid frontmatter in '{}': {error}", path.display()))
        })?;
        Ok(Document {
            id: frontmatter.id,
            uri: frontmatter.uri,
            title: frontmatter.title,
            content: content.to_string(),
            metadata_json: frontmatter.metadata_json,
            created_at: frontmatter.created_at,
            updated_at: frontmatter.updated_at,
            wing: frontmatter.wing,
            room: frontmatter.room,
            source_file: frontmatter.source_file,
            layer: frontmatter.layer,
            kind: frontmatter.kind,
            content_hash: frontmatter.content_hash,
            status: frontmatter.status,
            pinned: frontmatter.pinned,
            boost: frontmatter.boost,
            revision: frontmatter.revision,
        })
    }

    fn find_document(&self, id: &str) -> Result<Option<(PathBuf, Document)>, AppError> {
        let mut found = None;
        for path in self.markdown_files()? {
            let document = Self::read_document(&path)?;
            if document.id == id {
                if found.is_some() {
                    return Err(AppError::conflict(format!(
                        "duplicate markdown documents use id '{id}'"
                    )));
                }
                found = Some((path, document));
            }
        }
        Ok(found)
    }
}

impl Storage for MarkdownVaultStorage {
    fn metadata(&self) -> BackendMetadata {
        BackendMetadata {
            kind: BackendKind::Markdown,
            name: "markdown",
            capabilities: CAPABILITIES,
        }
    }

    fn upsert_document(&self, document: &Document) -> Result<(), AppError> {
        if document.id.is_empty() {
            return Err(AppError::config("markdown document id must not be empty"));
        }
        serde_json::from_str::<serde_json::Value>(&document.metadata_json).map_err(|error| {
            AppError::config(format!("document metadata_json is not valid JSON: {error}"))
        })?;
        let previous_path = self.find_document(&document.id)?.map(|(path, _)| path);
        let destination = self.document_path(document);
        let parent = destination
            .parent()
            .ok_or_else(|| AppError::config("markdown document path has no parent"))?;
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        if !parent.starts_with(&self.root) {
            return Err(AppError::forbidden("markdown document path escapes vault root"));
        }

        let frontmatter = Frontmatter {
            id: document.id.clone(),
            uri: document.uri.clone(),
            title: document.title.clone(),
            metadata_json: document.metadata_json.clone(),
            created_at: document.created_at,
            updated_at: document.updated_at,
            wing: document.wing.clone(),
            room: document.room.clone(),
            source_file: document.source_file.clone(),
            layer: document.layer.clone(),
            kind: document.kind.clone(),
            content_hash: document.content_hash.clone(),
            status: document.status.clone(),
            pinned: document.pinned,
            boost: document.boost,
            revision: document.revision,
        };
        let serialized = serde_json::to_string_pretty(&frontmatter)?;
        let body = format!("---\n{serialized}\n---\n{}", document.content);
        let temporary = parent.join(format!(".{}.{}.tmp", encode_component(&document.id), uuid::Uuid::new_v4()));
        fs::write(&temporary, body)?;
        fs::rename(&temporary, &destination)?;

        if let Some(old_path) = previous_path {
            if old_path != destination {
                fs::remove_file(old_path)?;
            }
        }
        Ok(())
    }

    fn get_document(&self, id: &str) -> Result<Option<Document>, AppError> {
        Ok(self.find_document(id)?.map(|(_, document)| document))
    }

    fn list_documents(&self) -> Result<Vec<Document>, AppError> {
        self.markdown_files()?
            .iter()
            .map(|path| Self::read_document(path))
            .collect()
    }

    fn delete_document(&self, id: &str) -> Result<bool, AppError> {
        let Some((path, _)) = self.find_document(id)? else {
            return Ok(false);
        };
        fs::remove_file(path)?;
        Ok(true)
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    if encoded.is_empty() || encoded == "." || encoded == ".." {
        format!("_{encoded}")
    } else {
        encoded
    }
}
