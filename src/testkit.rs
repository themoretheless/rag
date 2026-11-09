//! Shared fixtures for unit tests and, behind the `testkit` feature, for the
//! integration tests in `tests/it`.
//!
//! One place builds seeded documents and the baseline test `Config`, so a new
//! `Document`/`Chunk`/`Config` field is a single edit. Per-module helpers stay
//! as thin wrappers that name their deliberate differences (kind, hash, scope,
//! chunk sizes) through the builder or struct-update syntax.
//!
//! Not part of the supported API: the module only exists under `cfg(test)` or
//! `--features testkit`.

use std::cell::RefCell;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::db::Store;
use crate::models::{Chunk, Document};
use crate::util::content_hash;

/// A document with exactly one chunk, inserted through the plain Store API
/// (no graph extraction, no FTS refresh).
pub struct SeedDoc<'a> {
    id: &'a str,
    title: &'a str,
    content: &'a str,
    uri: Option<&'a str>,
    layer: &'a str,
    kind: Option<&'a str>,
    wing: Option<&'a str>,
    room: Option<&'a str>,
    source_file: Option<&'a str>,
    embedding: Vec<f32>,
    with_hash: bool,
    created_at: Option<DateTime<Utc>>,
}

impl<'a> SeedDoc<'a> {
    /// `layer=raw`, `kind=document`, `uri=text://{id}`, embedding `[0.1; 8]`, no hash.
    pub fn new(id: &'a str, title: &'a str, content: &'a str) -> Self {
        Self {
            id,
            title,
            content,
            uri: None,
            layer: "raw",
            kind: None,
            wing: None,
            room: None,
            source_file: None,
            embedding: vec![0.1; 8],
            with_hash: false,
            created_at: None,
        }
    }

    pub fn layer(mut self, layer: &'a str) -> Self {
        self.layer = layer;
        self
    }

    /// Explicit kind; without it, `wiki` layer documents get `kind=wiki`.
    pub fn kind(mut self, kind: &'a str) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn uri(mut self, uri: &'a str) -> Self {
        self.uri = Some(uri);
        self
    }

    pub fn scope(
        mut self,
        wing: Option<&'a str>,
        room: Option<&'a str>,
        source_file: Option<&'a str>,
    ) -> Self {
        self.wing = wing;
        self.room = room;
        self.source_file = source_file;
        self
    }

    pub fn embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = embedding;
        self
    }

    /// Store `content_hash(content)` on the document row.
    pub fn hashed(mut self) -> Self {
        self.with_hash = true;
        self
    }

    /// Fixed `created_at`/`updated_at` (default: now) for order-sensitive tests.
    pub fn created_at(mut self, at: DateTime<Utc>) -> Self {
        self.created_at = Some(at);
        self
    }

    pub fn insert(self, store: &Store) {
        let now = self.created_at.unwrap_or_else(Utc::now);
        let kind = self.kind.unwrap_or(if self.layer == "wiki" {
            "wiki"
        } else {
            "document"
        });
        store
            .upsert_document(&Document {
                id: self.id.into(),
                uri: self
                    .uri
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("text://{}", self.id)),
                title: self.title.into(),
                content: self.content.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                wing: self.wing.map(Into::into),
                room: self.room.map(Into::into),
                source_file: self.source_file.map(Into::into),
                layer: self.layer.into(),
                kind: kind.into(),
                content_hash: self.with_hash.then(|| content_hash(self.content)),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("{}-c0", self.id),
                document_id: self.id.into(),
                chunk_index: 0,
                content: self.content.into(),
                embedding: self.embedding,
                char_start: 0,
                char_end: self.content.len() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
}

thread_local! {
    static SCRATCH_DIRS: RefCell<Vec<tempfile::TempDir>> = const { RefCell::new(Vec::new()) };
}

/// A temporary directory removed when the calling test's thread ends.
///
/// For fixtures that return only handles (state, store) and cannot hand the
/// `TempDir` guard back; `tempdir().keep()` there leaked one database per test
/// run into the system temp directory.
pub fn scratch_dir() -> PathBuf {
    let dir = tempfile::tempdir().expect("create scratch dir");
    let path = dir.path().to_path_buf();
    SCRATCH_DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    path
}

/// A fresh Store in a [`scratch_dir`], removed when the test's thread ends.
pub fn temp_store() -> Store {
    Store::open(&scratch_dir().join("test.duckdb")).expect("open temp store")
}

/// A fresh Store plus the guard that owns its directory, for tests that also
/// need the directory (extra files, a second open, `Config::db_path`).
pub fn open_temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("test.duckdb")).expect("open store");
    (dir, store)
}

/// Baseline config for tests that embed with the mock provider and must not
/// reach an LLM: `embedding_model = "mock"`, `llm_enabled = false`, everything
/// else from [`Config::for_tests`]. Override the rest with struct-update syntax.
pub fn test_config(db_path: PathBuf, dims: usize) -> Config {
    Config {
        db_path,
        embedding_model: "mock".into(),
        embedding_dims: dims,
        llm_enabled: false,
        ..Config::for_tests()
    }
}
