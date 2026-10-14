//! Index markdown rendering and small shared helpers.

use super::*;

pub(super) fn resolve_doc(store: &Store, id_or_uri: &str) -> Result<Document> {
    let s = id_or_uri.trim();
    if let Some(d) = store.get_document(s)? {
        return Ok(d);
    }
    if let Some(d) = store.find_by_uri(s)? {
        return Ok(d);
    }
    Err(AppError::not_found(format!(
        "document not found: {id_or_uri}"
    )))
}

pub(super) fn slugify(s: &str) -> String {
    shared_slugify(s, SlugPolicy::WikiPage)
}

pub(super) use crate::util::first_line;
