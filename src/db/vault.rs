//! Git-friendly Markdown vault export.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;

use super::Store;
use crate::error::{AppError, Result};
use crate::models::GraphFilter;

pub const VAULT_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct VaultExportReport {
    pub success: bool,
    pub dry_run: bool,
    pub path: String,
    pub documents: u64,
    pub graph_nodes: u64,
    pub graph_edges: u64,
    pub files: u64,
    pub replaced_path: Option<String>,
}

#[derive(Serialize)]
struct VaultManifest<'a> {
    format: &'static str,
    version: u32,
    exported_at: String,
    documents: usize,
    embedding_manifest: &'a Option<crate::models::EmbeddingManifest>,
}

impl Store {
    /// Export human-readable Markdown plus machine-readable graph metadata.
    ///
    /// A complete vault is assembled in a sibling staging directory and then
    /// renamed into place. Explicit overwrite keeps the prior vault as a dated
    /// sibling instead of deleting it.
    pub fn export_vault(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<VaultExportReport> {
        let documents = self.list_documents()?;
        let graph = self.get_graph_view(GraphFilter::default())?;
        let manifest = self.get_embedding_manifest()?;
        let ops = self.list_ops_log(500)?;
        let files = documents.len() as u64 + 3;
        let exists = path.exists();
        if exists && !overwrite {
            return Err(AppError::conflict(format!(
                "vault destination '{}' already exists; set overwrite=true explicitly",
                path.display()
            )));
        }
        if dry_run {
            return Ok(VaultExportReport {
                success: true,
                dry_run,
                path: path.display().to_string(),
                documents: documents.len() as u64,
                graph_nodes: graph.nodes.len() as u64,
                graph_edges: graph.edges.len() as u64,
                files,
                replaced_path: None,
            });
        }

        let parent = path
            .parent()
            .ok_or_else(|| AppError::config("vault path needs a parent directory"))?;
        fs::create_dir_all(parent)?;
        let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("vault");
        let stage = parent.join(format!(".{name}.staging-{}", std::process::id()));
        if stage.exists() {
            return Err(AppError::conflict(format!(
                "staging path '{}' already exists",
                stage.display()
            )));
        }
        fs::create_dir(&stage)?;

        let result = (|| -> Result<()> {
            for document in &documents {
                let rel = document_path(document);
                let target = stage.join(rel);
                if let Some(dir) = target.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::write(target, markdown_document(document)?)?;
            }
            let meta = stage.join(".rag");
            fs::create_dir_all(&meta)?;
            fs::write(meta.join("graph.json"), serde_json::to_vec_pretty(&graph)?)?;
            fs::write(meta.join("ops-log.json"), serde_json::to_vec_pretty(&ops)?)?;
            fs::write(
                meta.join("manifest.json"),
                serde_json::to_vec_pretty(&VaultManifest {
                    format: "rag-markdown-vault",
                    version: VAULT_VERSION,
                    exported_at: Utc::now().to_rfc3339(),
                    documents: documents.len(),
                    embedding_manifest: &manifest,
                })?,
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }

        let replaced = if exists {
            let previous = available_previous_path(path);
            fs::rename(path, &previous)?;
            Some(previous)
        } else {
            None
        };
        if let Err(error) = fs::rename(&stage, path) {
            if let Some(previous) = &replaced {
                let _ = fs::rename(previous, path);
            }
            let _ = fs::remove_dir_all(&stage);
            return Err(error.into());
        }

        Ok(VaultExportReport {
            success: true,
            dry_run: false,
            path: path.display().to_string(),
            documents: documents.len() as u64,
            graph_nodes: graph.nodes.len() as u64,
            graph_edges: graph.edges.len() as u64,
            files,
            replaced_path: replaced.map(|p| p.display().to_string()),
        })
    }
}

fn document_path(document: &crate::models::Document) -> PathBuf {
    let wing = safe_segment(document.wing.as_deref().unwrap_or("unfiled"));
    let room = safe_segment(document.room.as_deref().unwrap_or("unfiled"));
    let layer = safe_segment(&document.layer);
    let title = safe_segment(&document.title);
    let id = safe_segment(&document.id.chars().take(12).collect::<String>());
    PathBuf::from("projects")
        .join(wing)
        .join(room)
        .join(layer)
        .join(format!("{title}--{id}.md"))
}

fn safe_segment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    if out.is_empty() {
        "untitled".into()
    } else {
        out.chars().take(96).collect()
    }
}

fn markdown_document(document: &crate::models::Document) -> Result<String> {
    let scalar = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    Ok(format!(
        "---\nid: {}\nuri: {}\ntitle: {}\nwing: {}\nroom: {}\nlayer: {}\nkind: {}\nstatus: {}\npinned: {}\nboost: {}\nrevision: {}\ncontent_hash: {}\nsource_file: {}\ncreated_at: {}\nupdated_at: {}\nmetadata_json: {}\n---\n\n{}\n",
        scalar(&document.id), scalar(&document.uri), scalar(&document.title),
        scalar(document.wing.as_deref().unwrap_or("")), scalar(document.room.as_deref().unwrap_or("")),
        scalar(&document.layer), scalar(&document.kind),
        scalar(&document.status), document.pinned, document.boost, document.revision,
        scalar(document.content_hash.as_deref().unwrap_or("")), scalar(document.source_file.as_deref().unwrap_or("")),
        scalar(&document.created_at.to_rfc3339()), scalar(&document.updated_at.to_rfc3339()),
        scalar(&document.metadata_json), document.content
    ))
}

fn available_previous_path(path: &Path) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let base = format!(
        "{}.previous-{stamp}",
        path.file_name().and_then(|v| v.to_str()).unwrap_or("vault")
    );
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut candidate = parent.join(&base);
    let mut suffix = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{base}-{suffix}"));
        suffix += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;
    use crate::models::Document;
    use crate::util::content_hash;

    #[test]
    fn vault_is_project_partitioned_and_overwrite_is_recoverable() {
        let temp = TempDir::new().unwrap();
        let store = Store::open(&temp.path().join("rag.duckdb")).unwrap();
        let content = "# Hello\n\nBody [[Other]].";
        store
            .upsert_document(&Document {
                id: "doc-123456789012345".into(),
                uri: "wiki://hello".into(),
                title: "Hello / World".into(),
                content: content.into(),
                metadata_json: "{}".into(),
                content_hash: Some(content_hash(content)),
                wing: Some("project-a".into()),
                room: Some("docs".into()),
                source_file: None,
                layer: "wiki".into(),
                kind: "article".into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                status: "active".into(),
                pinned: false,
                boost: 1.0,
                revision: 1,
            })
            .unwrap();
        let vault = temp.path().join("vault");
        let preview = store.export_vault(&vault, true, false).unwrap();
        assert_eq!(preview.documents, 1);
        assert!(!vault.exists());
        let first = store.export_vault(&vault, false, false).unwrap();
        assert!(first.success);
        let note = vault.join("projects/project-a/docs/wiki/Hello-World--doc-12345678.md");
        let body = fs::read_to_string(note).unwrap();
        assert!(body.contains("uri: \"wiki://hello\""));
        assert!(body.ends_with(content.to_owned().as_str()) || body.contains("Body [[Other]]."));
        assert!(vault.join(".rag/graph.json").is_file());
        assert!(vault.join(".rag/manifest.json").is_file());
        let second = store.export_vault(&vault, false, true).unwrap();
        assert!(Path::new(second.replaced_path.as_deref().unwrap()).is_dir());
    }
}
