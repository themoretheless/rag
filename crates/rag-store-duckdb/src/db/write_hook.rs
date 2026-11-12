//! Document write hooks: a seam for layers above the store to normalize a
//! document inside the store's write transaction, before the row is written.
//!
//! The store does not know who registers a hook. The composition root
//! (`storage::open_configured*`) and the producers of hook-relevant metadata
//! register the code-index source hook; registration is process-wide and
//! idempotent per hook name.

use std::borrow::Cow;
use std::sync::{Arc, RwLock};

use super::lin_engine::LinWriter;
use crate::error::Result;
use crate::models::Document;

/// Object-safe writer handle exposed to [`DocumentWriteHook`] inside a store transaction.
pub trait HookWriter {
    /// Insert or replace a record by primary key inside `col` within the active transaction.
    fn upsert_record(
        &mut self,
        col: &str,
        id: &str,
        fields: Vec<(String, lin::Value)>,
    ) -> Result<()>;
}

impl<W: LinWriter + ?Sized> HookWriter for W {
    fn upsert_record(
        &mut self,
        col: &str,
        id: &str,
        fields: Vec<(String, lin::Value)>,
    ) -> Result<()> {
        self.upsert_by_id(col, id, fields)
    }
}

/// Runs inside the document upsert transaction, before the CAS check and the
/// row write. The returned document is the one that gets written.
pub trait DocumentWriteHook: Send + Sync + 'static {
    /// Stable name; a second registration under the same name is ignored.
    fn name(&self) -> &'static str;

    /// Normalize `doc` using `writer` (the open write transaction). Writes made
    /// through `writer` commit or roll back with the document write.
    fn before_document_write<'a>(
        &self,
        writer: &mut dyn HookWriter,
        doc: &'a Document,
    ) -> Result<Cow<'a, Document>>;
}

static HOOKS: RwLock<Vec<Arc<dyn DocumentWriteHook>>> = RwLock::new(Vec::new());

/// Register `hook` for every store in this process. Idempotent by name.
pub fn register_document_write_hook(hook: Arc<dyn DocumentWriteHook>) {
    let mut hooks = HOOKS.write().unwrap_or_else(|e| e.into_inner());
    if hooks.iter().any(|h| h.name() == hook.name()) {
        return;
    }
    hooks.push(hook);
}

/// Apply the registered hooks in registration order.
pub(crate) fn run_document_write_hooks<'a>(
    writer: &mut dyn HookWriter,
    doc: &'a Document,
) -> Result<Cow<'a, Document>> {
    let hooks: Vec<Arc<dyn DocumentWriteHook>> = HOOKS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect();
    let mut current = Cow::Borrowed(doc);
    for hook in hooks {
        current = match current {
            Cow::Borrowed(d) => hook.before_document_write(writer, d)?,
            Cow::Owned(d) => Cow::Owned(hook.before_document_write(writer, &d)?.into_owned()),
        };
    }
    Ok(current)
}
