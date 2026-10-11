//! Durable, local review drafts. Acceptance participates in the wiki transaction.
use crate::{
    db::Store,
    error::{AppError, Result},
    models::Document,
    util::content_hash,
};
use duckdb::{params, Connection, OptionalExt};
use serde::{Deserialize, Serialize};

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

fn read(conn: &Connection, id: &str) -> Result<Proposal> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM wiki_proposals WHERE id = ?",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(&payload.ok_or_else(|| {
        AppError::not_found("proposal not found")
    })?)?)
}
fn save(conn: &Connection, proposal: &Proposal) -> Result<()> {
    conn.execute("INSERT INTO wiki_proposals VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET payload = excluded.payload", params![proposal.id, serde_json::to_string(proposal)?])?;
    Ok(())
}
pub fn get(store: &Store, id: &str) -> Result<Proposal> {
    read(&*store.lock()?, id)
}
pub fn list(store: &Store) -> Result<Vec<Proposal>> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare("SELECT payload FROM wiki_proposals ORDER BY id")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}
pub fn create(store: &Store, document_id: &str) -> Result<Proposal> {
    let base = crate::wiki::get_wiki_page(store, document_id)?;
    let mut sources = Vec::new();
    for (id, uri) in crate::wiki::review_parent_ids(store, &base)? {
        let source = store.get_document(&id)?;
        sources.push(ReviewSource {
            document_id: id,
            uri: source.as_ref().map(|d| d.uri.clone()).unwrap_or(uri),
            content_hash: source.as_ref().map(|d| content_hash(&d.content)),
            content: source.map(|d| d.content),
        });
    }
    let proposal = Proposal {
        id: uuid::Uuid::new_v4().to_string(),
        content: base.content.clone(),
        base,
        sources,
        revision: 1,
        status: "pending".into(),
    };
    save(&*store.lock()?, &proposal)?;
    Ok(proposal)
}
pub fn update(
    store: &Store,
    id: &str,
    revision: i64,
    content: String,
    reject: bool,
) -> Result<Proposal> {
    let conn = store.lock()?;
    let mut proposal = read(&conn, id)?;
    if proposal.status != "pending" || proposal.revision != revision {
        return Err(AppError::conflict("proposal changed; reload it"));
    }
    proposal.content = content;
    proposal.revision += 1;
    if reject {
        proposal.status = "rejected".into();
    }
    save(&conn, &proposal)?;
    Ok(proposal)
}

/// Called with the document transaction open. Any later error rolls acceptance back.
pub(crate) fn accept_locked(
    conn: &Connection,
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
    let mut proposal = read(conn, id)?;
    if proposal.status != "pending"
        || payload["proposal_revision"].as_i64() != Some(proposal.revision)
        || proposal.base.id != doc.id
        || proposal.base.uri != doc.uri
        || proposal.content != doc.content
    {
        return Err(AppError::conflict("proposal changed; reload it"));
    }
    let revision: Option<i64> = conn
        .query_row(
            "SELECT revision FROM documents WHERE id = ?",
            [&doc.id],
            |r| r.get(0),
        )
        .optional()?;
    if revision != Some(proposal.base.revision) {
        return Err(AppError::conflict("article changed after review started"));
    }
    for source in &proposal.sources {
        let body: Option<String> = conn
            .query_row(
                "SELECT content FROM documents WHERE id = ? OR (? <> '' AND uri = ?) ORDER BY CASE WHEN id = ? THEN 0 ELSE 1 END LIMIT 1",
                params![source.document_id, source.uri, source.uri, source.document_id],
                |r| r.get(0),
            )
            .optional()?;
        if body.as_deref().map(content_hash) != source.content_hash {
            return Err(AppError::conflict("source changed after review started"));
        }
    }
    proposal.status = "accepted".into();
    proposal.revision += 1;
    save(conn, &proposal)
}
