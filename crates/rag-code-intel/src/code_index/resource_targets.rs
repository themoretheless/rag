//! Stored non-code resource identities, validated against the published corpus.
use super::*;
use duckdb::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Fresh,
    Stale,
    Unpublished,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTarget {
    pub document_id: String,
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub content_hash: Option<String>,
    pub published_revision: Option<i64>,
    pub published_hash: Option<String>,
    pub state: TargetState,
    pub reason: String,
}
impl ResourceTarget {
    pub(super) fn frozen(
        id: &str,
        path: &str,
        project: Option<String>,
        revision: i64,
        hash: String,
    ) -> Self {
        Self {
            document_id: id.into(),
            path: path.into(),
            project,
            revision,
            content_hash: Some(hash.clone()),
            published_revision: Some(revision),
            published_hash: Some(hash),
            state: TargetState::Fresh,
            reason: "published_stored_source_identity".into(),
        }
    }
    pub(super) fn fresh(&self) -> bool {
        self.state == TargetState::Fresh
    }
}
pub(super) fn inventory(
    conn: &Connection,
    revision: Option<i64>,
) -> Result<Option<HashMap<String, ResourceTarget>>> {
    let Some(revision) = revision else {
        return Ok(None);
    };
    // Old revisions did not record attachment identity for non-code documents.
    // Neither missing/invalid inventories nor duplicate IDs qualify freshness.
    let value:Option<String>=conn.query_row("SELECT CAST(json_extract(try_cast(graph_json AS JSON),'$.file_resource_targets') AS VARCHAR) FROM code_index_revisions WHERE id=?",[revision],|r|r.get(0))?;
    let Some(value) = value else { return Ok(None) };
    let Ok(values) = serde_json::from_str::<Vec<ResourceTarget>>(&value) else {
        return Ok(None);
    };
    let mut out = HashMap::new();
    for value in values {
        if !value.fresh()
            || value.content_hash.is_none()
            || value.content_hash != value.published_hash
            || value.published_revision != Some(value.revision)
            || out.contains_key(&value.document_id)
        {
            return Ok(None);
        }
        out.insert(value.document_id.clone(), value);
    }
    Ok(Some(out))
}
// Actual table provenance is checked in addition to graph inventory metadata.
pub(super) struct Published {
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub hash: String,
}
pub(super) fn observe(
    current: ResourceTarget,
    inventory: Option<&HashMap<String, ResourceTarget>>,
    published: Option<Published>,
) -> ResourceTarget {
    let mut out = current;
    let Some(hash) = out.content_hash.as_deref() else {
        out.state = TargetState::Unavailable;
        out.reason = "stored_source_unavailable_or_corrupt".into();
        return out;
    };
    let Some(inventory) = inventory else {
        out.state = TargetState::Unpublished;
        out.reason = "published_resource_inventory_unavailable".into();
        return out;
    };
    let Some(captured) = inventory.get(&out.document_id) else {
        out.state = TargetState::Unpublished;
        out.reason = "source_attachment_not_in_published_inventory".into();
        return out;
    };
    out.published_revision = captured.published_revision;
    out.published_hash = captured.published_hash.clone();
    let table_matches = published.is_some_and(|p| {
        p.path == captured.path
            && p.project == captured.project
            && p.revision == captured.revision
            && Some(&p.hash) == captured.content_hash.as_ref()
    });
    if !table_matches
        || captured.path != out.path
        || captured.project != out.project
        || captured.revision != out.revision
        || captured.content_hash.as_deref() != Some(hash)
    {
        out.state = TargetState::Stale;
        out.reason = "stored_source_or_published_identity_changed".into();
    } else {
        out.state = TargetState::Fresh;
        out.reason = "published_stored_source_identity".into();
    }
    out
}

pub(super) fn diagnostics(targets: &[ResourceTarget], limit: usize) -> serde_json::Value {
    let unavailable: Vec<_> = targets.iter().filter(|t| !t.fresh()).collect();
    serde_json::json!({"items":unavailable.iter().take(limit).collect::<Vec<_>>(),"total":unavailable.len(),"truncated":unavailable.len()>limit,"targets_total":targets.len(),"fresh_targets":targets.len()-unavailable.len(),"hash_semantics":"decoded_stored_source_text","live_filesystem_checked":false,"binary_decoding_proven":false})
}

#[cfg(test)]
mod tests;
