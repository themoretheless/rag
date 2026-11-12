//! Filesystem source manifest and root-scoped preflight queries over `lin`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{MAIN_SEPARATOR, Path};

use chrono::Utc;

use super::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_i64, cell_opt_string, cell_str, cell_string,
};
use super::store::Store;
use crate::error::{AppError, Result};
use crate::util::format_db_timestamp;

pub use rag_mcp_core::SourceRootSummary;

#[cfg(any(test, feature = "testkit"))]
thread_local! {
    #[doc(hidden)]
    pub static TEST_FAIL_SOURCE_MANIFEST_UPSERT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Manifest state plus the joined document/chunk health needed by source sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceManifestEntry {
    pub canonical_path: String,
    pub canonical_root: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub content_hash: String,
    pub document_id: String,
    pub last_seen: Option<String>,
    pub last_synced: Option<String>,
    pub manifest_present: bool,
    pub document_exists: bool,
    pub chunks_exist: bool,
    pub document_indexable: bool,
    pub document_content_hash: Option<String>,
    pub document_metadata_json: Option<String>,
    pub document_wing: Option<String>,
    pub document_room: Option<String>,
}

/// Values published after a file has been synchronized successfully.
#[derive(Debug, Clone, Copy)]
pub struct SourceManifestWrite<'a> {
    pub canonical_path: &'a str,
    pub canonical_root: &'a str,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub content_hash: &'a str,
    pub document_id: &'a str,
}

impl Store {
    /// List synchronized roots, optionally restricted to one project (`wing`).
    pub fn list_source_roots(&self, project: Option<&str>) -> Result<Vec<SourceRootSummary>> {
        let project = project.map(str::trim).filter(|value| !value.is_empty());
        let r = self.read()?;
        let mut grouped: BTreeMap<String, (u64, Option<String>, Option<String>)> = BTreeMap::new();
        let mut wing_cache: HashMap<String, Option<String>> = HashMap::new();

        r.scan_each("source_manifest", |row| {
            if let Some(proj) = project {
                let doc_id = cell_str(row, "document_id").unwrap_or_default();
                let wing = if let Some(cached) = wing_cache.get(doc_id) {
                    cached.as_deref()
                } else {
                    let fetched = r
                        .get_by_id("documents", doc_id)?
                        .and_then(|d| cell_opt_string(&d, "wing"));
                    wing_cache.insert(doc_id.to_string(), fetched);
                    wing_cache.get(doc_id).and_then(|v| v.as_deref())
                };
                if wing != Some(proj) {
                    return Ok(true);
                }
            }
            let root = cell_string(row, "canonical_root");
            let seen = cell_opt_string(row, "last_seen");
            let synced = cell_opt_string(row, "last_synced");
            let entry = grouped.entry(root).or_insert((0, None, None));
            entry.0 += 1;
            if let Some(s) = seen {
                if entry.1.as_ref().is_none_or(|prev| &s > prev) {
                    entry.1 = Some(s);
                }
            }
            if let Some(sy) = synced {
                if entry.2.as_ref().is_none_or(|prev| &sy > prev) {
                    entry.2 = Some(sy);
                }
            }
            Ok(true)
        })?;

        Ok(grouped
            .into_iter()
            .map(
                |(canonical_root, (file_count, last_seen, last_synced))| SourceRootSummary {
                    canonical_root,
                    file_count,
                    last_seen,
                    last_synced,
                },
            )
            .collect())
    }

    /// Preload all manifest and legacy source-document state under one root.
    ///
    /// Joined existence flags eliminate per-file document/chunk probes on the
    /// unchanged fast path. Legacy source documents are included once so the
    /// first sync can seed their manifest without losing metadata.
    pub fn load_source_manifest_root(
        &self,
        canonical_root: &Path,
    ) -> Result<HashMap<String, SourceManifestEntry>> {
        let root = canonical_root.display().to_string();
        let prefix = root_prefix(&root);
        let r = self.read()?;

        struct RootDoc {
            id: String,
            source_file: String,
            indexable: bool,
            content_hash: Option<String>,
            metadata_json: String,
            wing: Option<String>,
            room: Option<String>,
        }

        let mut root_docs_by_id: HashMap<String, RootDoc> = HashMap::new();
        r.scan_each_projected(
            "documents",
            &[
                "id",
                "source_file",
                "content",
                "content_hash",
                "metadata_json",
                "wing",
                "room",
            ],
            |row| {
                let Some(sf) = cell_str(row, "source_file") else {
                    return Ok(true);
                };
                if sf == root || sf.starts_with(&prefix) {
                    let id = cell_string(row, "id");
                    let content = cell_str(row, "content").unwrap_or_default();
                    root_docs_by_id.insert(
                        id.clone(),
                        RootDoc {
                            id,
                            source_file: sf.to_string(),
                            indexable: !content.trim().is_empty(),
                            content_hash: cell_opt_string(row, "content_hash"),
                            metadata_json: cell_string(row, "metadata_json"),
                            wing: cell_opt_string(row, "wing"),
                            room: cell_opt_string(row, "room"),
                        },
                    );
                }
                Ok(true)
            },
        )?;

        let mut all_manifest_paths: HashSet<String> = HashSet::new();
        let mut all_manifest_docs: HashSet<String> = HashSet::new();
        let mut root_manifests: Vec<lin::Row> = Vec::new();

        r.scan_each("source_manifest", |row| {
            let cp = cell_string(row, "canonical_path");
            let cr = cell_str(row, "canonical_root").unwrap_or_default();
            let did = cell_string(row, "document_id");
            all_manifest_paths.insert(cp.clone());
            if !did.is_empty() {
                all_manifest_docs.insert(did);
            }
            if cr == root || cp == root || cp.starts_with(&prefix) {
                root_manifests.push(row.clone());
            }
            Ok(true)
        })?;

        root_manifests
            .sort_by(|a, b| cell_str(a, "canonical_path").cmp(&cell_str(b, "canonical_path")));

        let mut entries: HashMap<String, SourceManifestEntry> = HashMap::new();
        let mut chunks_cache: HashMap<String, bool> = HashMap::new();
        let mut has_chunks = |doc_id: &str| -> Result<bool> {
            if let Some(&cached) = chunks_cache.get(doc_id) {
                return Ok(cached);
            }
            let mut exists = false;
            r.for_each_by_eq_projected("chunks", "document_id", doc_id, Some(&["id"]), |_| {
                exists = true;
                Ok(false)
            })?;
            chunks_cache.insert(doc_id.to_string(), exists);
            Ok(exists)
        };

        for row in &root_manifests {
            let canonical_path = cell_string(row, "canonical_path");
            if entries.contains_key(&canonical_path) {
                continue;
            }
            let document_id = cell_string(row, "document_id");
            let doc_opt = root_docs_by_id.get(&document_id);
            let chunks_exist = has_chunks(&document_id)?;
            let size_bytes = cell_i64(row, "size_bytes", 0).max(0) as u64;
            entries.insert(
                canonical_path.clone(),
                SourceManifestEntry {
                    canonical_path,
                    canonical_root: cell_string(row, "canonical_root"),
                    size_bytes,
                    mtime_ns: cell_i64(row, "mtime_ns", 0),
                    content_hash: cell_string(row, "content_hash"),
                    document_id,
                    last_seen: cell_opt_string(row, "last_seen"),
                    last_synced: cell_opt_string(row, "last_synced"),
                    manifest_present: true,
                    document_exists: doc_opt.is_some(),
                    chunks_exist,
                    document_indexable: doc_opt.is_some_and(|d| d.indexable),
                    document_content_hash: doc_opt.and_then(|d| d.content_hash.clone()),
                    document_metadata_json: doc_opt.map(|d| d.metadata_json.clone()),
                    document_wing: doc_opt.and_then(|d| d.wing.clone()),
                    document_room: doc_opt.and_then(|d| d.room.clone()),
                },
            );
        }

        let mut legacy_docs: Vec<&RootDoc> = root_docs_by_id
            .values()
            .filter(|d| {
                !all_manifest_paths.contains(&d.source_file) && !all_manifest_docs.contains(&d.id)
            })
            .collect();
        legacy_docs.sort_by(|a, b| {
            a.source_file
                .cmp(&b.source_file)
                .then_with(|| a.id.cmp(&b.id))
        });

        for d in legacy_docs {
            if entries.contains_key(&d.source_file) {
                continue;
            }
            let chunks_exist = has_chunks(&d.id)?;
            entries.insert(
                d.source_file.clone(),
                SourceManifestEntry {
                    canonical_path: d.source_file.clone(),
                    canonical_root: root.clone(),
                    size_bytes: 0,
                    mtime_ns: 0,
                    content_hash: d.content_hash.clone().unwrap_or_default(),
                    document_id: d.id.clone(),
                    last_seen: None,
                    last_synced: None,
                    manifest_present: false,
                    document_exists: true,
                    chunks_exist,
                    document_indexable: d.indexable,
                    document_content_hash: d.content_hash.clone(),
                    document_metadata_json: Some(d.metadata_json.clone()),
                    document_wing: d.wing.clone(),
                    document_room: d.room.clone(),
                },
            );
        }

        Ok(entries)
    }

    /// Publish metadata/hash/document linkage after a successful sync.
    pub fn upsert_source_manifest(&self, entry: SourceManifestWrite<'_>) -> Result<()> {
        let mut w = self.write()?;
        upsert_source_manifest_locked(&mut w, entry)
    }

    /// Upsert several manifest rows in one transaction (all or nothing).
    pub fn upsert_source_manifests(&self, entries: &[SourceManifestWrite<'_>]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.with_tx(|tx| {
            for entry in entries {
                upsert_source_manifest_locked(tx, *entry)?;
            }
            Ok(())
        })
    }

    /// Update `last_seen` for a scanned root in bounded batches.
    pub fn mark_source_manifest_seen(
        &self,
        canonical_root: &Path,
        canonical_paths: &[String],
    ) -> Result<()> {
        if canonical_paths.is_empty() {
            return Ok(());
        }
        let root = canonical_root.display().to_string();
        let now_s = format_db_timestamp(Utc::now());
        let mut w = self.write()?;
        for path in canonical_paths {
            let Some(existing) = w.get_by_id("source_manifest", path)? else {
                continue;
            };
            if cell_str(&existing, "canonical_root") != Some(root.as_str()) {
                continue;
            }
            w.update_where(
                "source_manifest",
                lin::query::pred::eq("id", path.as_str()),
                RecBuilder::new().str("last_seen", &now_s).build(),
            )?;
        }
        Ok(())
    }

    /// Delete one manifest record. Returns true when a row was removed.
    pub fn delete_source_manifest(&self, canonical_path: &str) -> Result<bool> {
        let mut w = self.write()?;
        let by_id = w.delete_by_id("source_manifest", canonical_path)?;
        let by_col = w.delete_where(
            "source_manifest",
            lin::query::pred::eq("canonical_path", canonical_path),
        )?;
        Ok(by_id || by_col > 0)
    }
}

pub(super) fn upsert_source_manifest_locked(
    w: &mut impl LinWriter,
    entry: SourceManifestWrite<'_>,
) -> Result<()> {
    #[cfg(any(test, feature = "testkit"))]
    if TEST_FAIL_SOURCE_MANIFEST_UPSERT.with(|f| f.get()) {
        return Err(AppError::db(
            "source_manifest table unavailable (injected fault)",
        ));
    }
    let size_bytes = i64::try_from(entry.size_bytes)
        .map_err(|_| AppError::config("source file size exceeds BIGINT"))?;
    let now_s = format_db_timestamp(Utc::now());
    let rec = RecBuilder::new()
        .id(entry.canonical_path)
        .str("canonical_path", entry.canonical_path)
        .str("canonical_root", entry.canonical_root)
        .i64("size_bytes", size_bytes)
        .i64("mtime_ns", entry.mtime_ns)
        .str("content_hash", entry.content_hash)
        .str("document_id", entry.document_id)
        .str("last_seen", &now_s)
        .str("last_synced", &now_s)
        .build();
    w.upsert("source_manifest", rec)
}

fn root_prefix(root: &str) -> String {
    if root.ends_with(MAIN_SEPARATOR) {
        root.to_string()
    } else {
        format!("{root}{MAIN_SEPARATOR}")
    }
}
