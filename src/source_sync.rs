//! Application service for synchronizing a filesystem tree into the corpus.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::file_ingest::{extract_file, is_supported_source, merge_metadata};
use crate::ingest::{IngestCommand, IngestService};
use crate::source_scan::{collect_source_files, SourceScanPolicy};
use crate::util::{check_path_allowlist, content_hash};

#[derive(Debug, Clone)]
pub struct SourceSyncCommand {
    pub path: PathBuf,
    pub remove_deleted: bool,
    pub wing: Option<String>,
    pub room: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SourceSyncError {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Default, Serialize)]
pub struct SourceSyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
    pub deleted: Vec<String>,
    pub errors: Vec<SourceSyncError>,
}

pub struct SourceSyncService<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
}

impl<'a> SourceSyncService<'a> {
    pub fn new(
        store: &'a Store,
        embedder: &'a Arc<dyn EmbeddingProvider>,
        config: &'a Config,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    pub async fn sync(&self, command: SourceSyncCommand) -> Result<SourceSyncReport, AppError> {
        check_path_allowlist(&command.path, &self.config.ingest_roots)?;
        let root = command.path.canonicalize().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found(format!("directory not found: {}", command.path.display()))
            } else {
                AppError::from(error)
            }
        })?;
        if !root.is_dir() {
            return Err(AppError::config(format!(
                "sync_sources path is not a directory: {}",
                root.display()
            )));
        }

        let files = collect_source_files(&root, &SourceScanPolicy::default())?;
        let mut seen = BTreeSet::new();
        let mut report = SourceSyncReport::default();
        for path in files {
            self.sync_file(&root, &path, &command, &mut seen, &mut report)
                .await;
        }
        if command.remove_deleted {
            self.remove_deleted(&root, &seen, &mut report)?;
        }
        Ok(report)
    }

    async fn sync_file(
        &self,
        root: &Path,
        path: &Path,
        command: &SourceSyncCommand,
        seen: &mut BTreeSet<String>,
        report: &mut SourceSyncReport,
    ) {
        if let Err(error) = self.try_sync_file(root, path, command, seen, report).await {
            report.errors.push(SourceSyncError {
                path: path.display().to_string(),
                error: error.to_string(),
            });
        }
    }

    async fn try_sync_file(
        &self,
        root: &Path,
        path: &Path,
        command: &SourceSyncCommand,
        seen: &mut BTreeSet<String>,
        report: &mut SourceSyncReport,
    ) -> Result<(), AppError> {
        let canonical = path.canonicalize()?;
        let source_file = canonical.display().to_string();
        seen.insert(source_file.clone());
        let uri = format!("file://{}", canonical.display());
        let extracted =
            extract_file(&canonical).map_err(|error| AppError::config(error.to_string()))?;
        let existing = self.store.find_by_uri(&uri)?;
        let hash = content_hash(&extracted.text);
        let unchanged = existing.as_ref().is_some_and(|document| {
            document
                .content_hash
                .as_deref()
                .map(|stored| stored == hash)
                .unwrap_or_else(|| content_hash(&document.content) == hash)
        });
        let has_chunks = existing
            .as_ref()
            .and_then(|document| self.store.list_chunks_for_document(&document.id).ok())
            .is_some_and(|chunks| !chunks.is_empty());
        if unchanged && has_chunks {
            report.skipped.push(source_file);
            return Ok(());
        }

        let metadata_json = merge_metadata(
            existing
                .as_ref()
                .map(|document| document.metadata_json.clone()),
            extracted.metadata,
        )
        .map_err(|error| AppError::config(error.to_string()))?;
        let (inferred_wing, inferred_room) = inferred_scope(root, &canonical);
        let result = IngestService::new(self.store, self.embedder, self.config)
            .ingest(IngestCommand {
                text: extracted.text,
                title: canonical
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string),
                uri: Some(uri),
                metadata_json: Some(metadata_json),
                wing: command
                    .wing
                    .clone()
                    .or_else(|| existing.as_ref().and_then(|d| d.wing.clone()))
                    .or(Some(inferred_wing)),
                room: command
                    .room
                    .clone()
                    .or_else(|| existing.as_ref().and_then(|d| d.room.clone()))
                    .or(Some(inferred_room)),
                source_file: Some(source_file.clone()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await?;
        if result.op == "inserted" {
            report.added.push(source_file);
        } else {
            report.updated.push(source_file);
        }
        Ok(())
    }

    fn remove_deleted(
        &self,
        root: &Path,
        seen: &BTreeSet<String>,
        report: &mut SourceSyncReport,
    ) -> Result<(), AppError> {
        for document in self.store.list_documents()? {
            let Some(source) = document.source_file else {
                continue;
            };
            let path = Path::new(&source);
            if path.starts_with(root)
                && is_supported_source(path)
                && !seen.contains(&source)
                && !path.exists()
            {
                match self.store.delete_by_source(&source) {
                    Ok(count) if count > 0 => report.deleted.push(source),
                    Ok(_) => {}
                    Err(error) => report.errors.push(SourceSyncError {
                        path: source,
                        error: error.to_string(),
                    }),
                }
            }
        }
        Ok(())
    }
}

fn inferred_scope(root: &Path, path: &Path) -> (String, String) {
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut components = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str());
    if root_name.eq_ignore_ascii_case("sources") {
        (
            components.next().unwrap_or("project").to_string(),
            components.next().unwrap_or("root").to_string(),
        )
    } else {
        let room = relative
            .parent()
            .and_then(|parent| parent.components().next())
            .and_then(|part| part.as_os_str().to_str())
            .unwrap_or("root")
            .to_string();
        (root_name.to_string(), room)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;

    #[tokio::test]
    async fn sync_is_idempotent_and_preserves_project_scope() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let project = root.path().join("alpha");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("README.md"), "# Alpha\nbody").unwrap();
        let config = Config {
            db_path: db.path().join("test.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let command = SourceSyncCommand {
            path: project,
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: Some("root".into()),
        };

        let first = service.sync(command.clone()).await.unwrap();
        let second = service.sync(command).await.unwrap();

        assert_eq!(first.added.len(), 1);
        assert_eq!(second.skipped.len(), 1);
        let document = store.list_documents().unwrap().pop().unwrap();
        assert_eq!(document.wing.as_deref(), Some("alpha"));
        assert_eq!(document.room.as_deref(), Some("root"));
    }
}
