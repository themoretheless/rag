//! Review proposals for a wiki page: snapshot its parent sources, then store
//! the draft through `db::review`.

use super::*;
use crate::db::review::{self, Proposal, ReviewSource};

/// Create a pending review proposal for wiki page `document_id`, capturing the
/// current content of every parent source it was compiled from.
pub fn create_review_proposal(store: &Store, document_id: &str) -> Result<Proposal> {
    let base = get_wiki_page(store, document_id)?;
    let mut sources = Vec::new();
    for (id, uri) in review_parent_ids(store, &base)? {
        let source = store.get_document(&id)?;
        sources.push(ReviewSource {
            document_id: id,
            uri: source.as_ref().map(|d| d.uri.clone()).unwrap_or(uri),
            content_hash: source.as_ref().map(|d| content_hash(&d.content)),
            content: source.map(|d| d.content),
        });
    }
    review::insert_new(store, base, sources)
}
