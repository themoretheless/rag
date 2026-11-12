//! Durable, local review drafts over `lin`. Acceptance participates in the wiki transaction.

use serde::{Deserialize, Serialize};

use super::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_i64_opt, cell_opt_string, cell_string,
};
use crate::{
    db::Store,
    error::{AppError, Result},
    models::Document,
    util::content_hash,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewSource {
    pub document_id: String,
    pub uri: String,
    pub content_hash: Option<String>,
    pub content: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub base: Document,
    pub sources: Vec<ReviewSource>,
    pub content: String,
    pub revision: i64,
    pub status: String,
}

fn read(r: &impl LinReader, id: &str) -> Result<Proposal> {
    let payload = r
        .get_by_id("wiki_proposals", id)?
        .and_then(|row| cell_opt_string(&row, "payload"))
        .ok_or_else(|| AppError::not_found("proposal not found"))?;
    Ok(serde_json::from_str(&payload)?)
}

fn save(w: &mut impl LinWriter, proposal: &Proposal) -> Result<()> {
    let payload = serde_json::to_string(proposal)?;
    let rec = RecBuilder::new()
        .id(&proposal.id)
        .str("payload", &payload)
        .build();
    w.upsert("wiki_proposals", rec)
}

/// Load one wiki review proposal by id.
pub fn get(store: &Store, id: &str) -> Result<Proposal> {
    let r = store.read()?;
    read(&r, id)
}

/// All wiki review proposals, ordered by id.
pub fn list(store: &Store) -> Result<Vec<Proposal>> {
    let r = store.read()?;
    let mut pairs = r.filter_map_rows("wiki_proposals", |row| {
        Ok(Some((cell_string(row, "id"), cell_string(row, "payload"))))
    })?;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
        .into_iter()
        .map(|(_, raw)| Ok(serde_json::from_str(&raw)?))
        .collect()
}

/// Store a new pending proposal for `base` with its source snapshots.
/// Building those snapshots is wiki policy (`wiki::create_review_proposal`).
pub fn insert_new(store: &Store, base: Document, sources: Vec<ReviewSource>) -> Result<Proposal> {
    let proposal = Proposal {
        id: uuid::Uuid::new_v4().to_string(),
        content: base.content.clone(),
        base,
        sources,
        revision: 1,
        status: "pending".into(),
    };
    let mut w = store.write()?;
    save(&mut w, &proposal)?;
    Ok(proposal)
}

/// Replace a pending proposal's content (optionally rejecting it); fails
/// with a conflict unless it is still pending at `revision`.
pub fn update(
    store: &Store,
    id: &str,
    revision: i64,
    content: String,
    reject: bool,
) -> Result<Proposal> {
    let mut w = store.write()?;
    let mut proposal = read(&w, id)?;
    if proposal.status != "pending" || proposal.revision != revision {
        return Err(AppError::conflict("proposal changed; reload it"));
    }
    proposal.content = content;
    proposal.revision += 1;
    if reject {
        proposal.status = "rejected".into();
    }
    save(&mut w, &proposal)?;
    Ok(proposal)
}

/// Called with the document transaction open. Any later error rolls acceptance back.
pub fn accept_locked(
    w: &mut impl LinWriter,
    doc: &Document,
    audit: &crate::models::OpsLogEntry,
) -> Result<()> {
    if audit.op != "wiki_review_accept" {
        return Ok(());
    }
    let payload: serde_json::Value = serde_json::from_str(&audit.payload_json)?;
    let id = payload["proposal_id"]
        .as_str()
        .ok_or_else(|| AppError::config("proposal id missing"))?;
    let mut proposal = read(w, id)?;
    if proposal.status != "pending"
        || payload["proposal_revision"].as_i64() != Some(proposal.revision)
        || proposal.base.id != doc.id
        || proposal.base.uri != doc.uri
        || proposal.content != doc.content
    {
        return Err(AppError::conflict("proposal changed; reload it"));
    }
    let revision = w
        .get_by_id("documents", &doc.id)?
        .and_then(|row| cell_i64_opt(&row, "revision"));
    if revision != Some(proposal.base.revision) {
        return Err(AppError::conflict("article changed after review started"));
    }
    for source in &proposal.sources {
        let body: Option<String> =
            if let Some(row) = w.get_by_id("documents", &source.document_id)? {
                Some(cell_string(&row, "content"))
            } else if !source.uri.is_empty() {
                w.find_by_eq("documents", "uri", &source.uri)?
                    .first()
                    .map(|row| cell_string(row, "content"))
            } else {
                None
            };
        if body.as_deref().map(content_hash) != source.content_hash {
            return Err(AppError::conflict("source changed after review started"));
        }
    }
    proposal.status = "accepted".into();
    proposal.revision += 1;
    save(w, &proposal)
}

#[cfg(test)]
mod tests;
