//! Shared fixtures for in-crate unit tests.
//!
//! One place builds seeded documents so a new `Document`/`Chunk` field is a
//! single edit. Per-module helpers stay as thin wrappers that name their
//! deliberate differences (kind, hash, scope) through the builder.

use std::cell::RefCell;
use std::path::PathBuf;

use chrono::Utc;

use crate::db::Store;
use crate::models::{Chunk, Document};
use crate::util::content_hash;

/// A document with exactly one chunk, inserted through the plain Store API
/// (no graph extraction, no FTS refresh).
pub(crate) struct SeedDoc<'a> {
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
}

impl<'a> SeedDoc<'a> {
    /// `layer=raw`, `kind=document`, `uri=text://{id}`, embedding `[0.1; 8]`, no hash.
    pub(crate) fn new(id: &'a str, title: &'a str, content: &'a str) -> Self {
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
        }
    }

    pub(crate) fn layer(mut self, layer: &'a str) -> Self {
        self.layer = layer;
        self
    }

    /// Explicit kind; without it, `wiki` layer documents get `kind=wiki`.
    pub(crate) fn kind(mut self, kind: &'a str) -> Self {
        self.kind = Some(kind);
        self
    }

    pub(crate) fn uri(mut self, uri: &'a str) -> Self {
        self.uri = Some(uri);
        self
    }

    pub(crate) fn scope(
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

    pub(crate) fn embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = embedding;
        self
    }

    /// Store `content_hash(content)` on the document row.
    pub(crate) fn hashed(mut self) -> Self {
        self.with_hash = true;
        self
    }

    pub(crate) fn insert(self, store: &Store) {
        let now = Utc::now();
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
pub(crate) fn scratch_dir() -> PathBuf {
    let dir = tempfile::tempdir().expect("create scratch dir");
    let path = dir.path().to_path_buf();
    SCRATCH_DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    path
}

/// A fresh Store in a [`scratch_dir`], removed when the test's thread ends.
pub(crate) fn temp_store() -> Store {
    Store::open(&scratch_dir().join("test.duckdb")).expect("open temp store")
}
