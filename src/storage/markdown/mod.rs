//! Markdown vault storage adapter.
//!
//! Each document is a Markdown file whose YAML 1.2 frontmatter is represented
//! by a JSON mapping (JSON is a valid YAML 1.2 subset). The files, rather than
//! a sidecar database, are the source of truth.

use std::ffi::OsStr;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AppError, Document};

use super::{BackendKind, BackendMetadata, Storage, StorageCapability};

const SIDECAR_SCHEMA_VERSION: u32 = 1;
const SIDECAR_DIRECTORY: &str = ".rag";
const SIDECAR_FILE: &str = "documents.v1.jsonl";

mod watcher;

pub use watcher::{VaultWatchConfig, VaultWatchReport};

pub(super) const CAPABILITIES: &[StorageCapability] = &[StorageCapability::Documents];

#[derive(Debug, Clone)]
pub struct MarkdownVaultStorage {
    root: PathBuf,
}

/// One rebuildable lexical index entry. Markdown and frontmatter remain the
/// source of truth; no content is read back from this sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultIndexEntry {
    pub schema_version: u32,
    pub document_id: String,
    pub relative_path: String,
    pub content_hash: String,
    pub title: String,
    pub layer: String,
    pub kind: String,
    pub word_count: usize,
    pub heading_count: usize,
    pub wikilink_count: usize,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultReindexReport {
    pub index_path: PathBuf,
    pub documents_indexed: usize,
    pub schema_version: u32,
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

    /// Rebuild `.rag/documents.v1.jsonl` from safe Markdown files in stable
    /// path order and atomically replace the previous sidecar.
    pub fn reindex(&self) -> Result<VaultReindexReport, AppError> {
        let mut entries = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for path in self.markdown_files()? {
            let canonical = fs::canonicalize(&path)?;
            if !canonical.starts_with(&self.root) {
                return Err(AppError::forbidden(format!(
                    "markdown index path '{}' escapes vault root",
                    path.display()
                )));
            }
            let relative = canonical.strip_prefix(&self.root).map_err(|_| {
                AppError::forbidden("markdown index path escapes vault root")
            })?;
            let relative_path = relative.to_str().ok_or_else(|| {
                AppError::config(format!(
                    "markdown index path '{}' is not valid UTF-8",
                    relative.display()
                ))
            })?;
            let document = Self::read_document(&canonical)?;
            if !ids.insert(document.id.clone()) {
                return Err(AppError::conflict(format!(
                    "duplicate markdown documents use id '{}'",
                    document.id
                )));
            }
            entries.push(VaultIndexEntry::from_document(
                relative_path,
                &document,
            ));
        }

        let destination = self.write_sidecar(&entries)?;

        Ok(VaultReindexReport {
            index_path: destination,
            documents_indexed: entries.len(),
            schema_version: SIDECAR_SCHEMA_VERSION,
        })
    }

    /// Watch the vault and keep the JSONL sidecar current until `stop` returns
    /// true. This is an explicit, blocking opt-in API; opening a Markdown or
    /// DuckDB backend never starts a watcher.
    pub fn watch_sidecar(
        &self,
        config: VaultWatchConfig,
        stop: impl Fn() -> bool,
    ) -> Result<VaultWatchReport, AppError> {
        watcher::watch(self, config, stop)
    }

    fn sidecar_path(&self) -> PathBuf {
        self.root.join(SIDECAR_DIRECTORY).join(SIDECAR_FILE)
    }

    fn write_sidecar(&self, entries: &[VaultIndexEntry]) -> Result<PathBuf, AppError> {
        let sidecar_directory = self.root.join(SIDECAR_DIRECTORY);
        fs::create_dir_all(&sidecar_directory)?;
        let destination = self.sidecar_path();
        let temporary = sidecar_directory.join(format!(
            ".{SIDECAR_FILE}.{}.tmp",
            uuid::Uuid::new_v4()
        ));
        let write_result = (|| -> Result<(), AppError> {
            let file = fs::File::create(&temporary)?;
            let mut writer = BufWriter::new(file);
            for entry in entries {
                serde_json::to_writer(&mut writer, entry)?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
            writer.get_ref().sync_all()?;
            fs::rename(&temporary, &destination)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result?;
        Ok(destination)
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

impl VaultIndexEntry {
    fn from_document(relative_path: &str, document: &Document) -> Self {
        let terms = document
            .title
            .split(|character: char| !character.is_alphanumeric())
            .chain(
                document
                    .content
                    .split(|character: char| !character.is_alphanumeric()),
            )
            .filter(|term| !term.is_empty())
            .map(|term| term.to_lowercase())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Self {
            schema_version: SIDECAR_SCHEMA_VERSION,
            document_id: document.id.clone(),
            relative_path: relative_path.replace(std::path::MAIN_SEPARATOR, "/"),
            content_hash: crate::util::content_hash(&document.content),
            title: document.title.clone(),
            layer: document.layer.clone(),
            kind: document.kind.clone(),
            word_count: document.content.split_whitespace().count(),
            heading_count: document
                .content
                .lines()
                .filter(|line| line.trim_start().starts_with('#'))
                .count(),
            wikilink_count: document.content.match_indices("[[").count(),
            terms,
        }
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
