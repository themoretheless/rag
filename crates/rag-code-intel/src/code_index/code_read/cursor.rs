//! Optimistic captured-view cursors; not authentication or retained snapshots.
use super::*;
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(super) enum IdInput {
    List(Vec<String>),
    Delimited(String),
}
pub(super) fn ids<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Vec<String>>, D::Error> {
    let value = Option::<IdInput>::deserialize(d)?;
    match value {
        None => Ok(None),
        Some(IdInput::List(v)) => Ok(Some(v)),
        Some(IdInput::Delimited(v)) => {
            let mut ids = Vec::new();
            for id in v
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .take(101)
            {
                if id.len() > 512 {
                    return Err(serde::de::Error::custom("code entry ID exceeds 512 bytes"));
                }
                ids.push(id.into());
            }
            Ok(Some(ids))
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Position {
    pub span: usize,
    pub byte: usize,
}
pub(super) struct View {
    query: String,
    view: String,
}
impl View {
    pub fn new(q: &CodeReadQuery, snapshot: &CodeSnapshot) -> Result<Self> {
        let ids: BTreeSet<_> = q.entry_ids.as_deref().unwrap_or(&[]).iter().collect();
        let query = content_hash(&serde_json::to_string(
            &serde_json::json!({"symbol":q.selection.symbol,"path":q.selection.path,"project":q.project,"document_id":q.document_id,"entry_ids":ids,"start":q.start,"end":q.end,"start_byte":q.start_byte,"end_byte":q.end_byte,"include_containers":q.include_containers,"source":q.source}),
        )?);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rag-code-view-1");
        hash.update(&serde_json::to_vec(&(
            snapshot.index_revision,
            INDEX_FORMAT_VERSION,
        ))?);
        let docs: BTreeMap<_, _> = snapshot.document_scopes.iter().collect();
        for (id, scope) in docs {
            hash.update(&serde_json::to_vec(&(
                id,
                &scope.path,
                &scope.project,
                snapshot.document_fingerprints.get(id),
            ))?);
        }
        for symbol in &snapshot.symbols {
            hash.update(&serde_json::to_vec(&(
                &symbol.id,
                &symbol.document_id,
                symbol.start_byte,
                symbol.end_byte,
                &symbol.build_context,
            ))?);
        }
        let mut stale = snapshot.stale_documents.clone();
        stale.sort();
        let mut unindexed = snapshot.unindexed_documents.clone();
        unindexed.sort();
        hash.update(&serde_json::to_vec(&(stale, unindexed))?);
        Ok(Self {
            query,
            view: hash.finalize().to_hex().to_string(),
        })
    }
    pub fn encode(&self, p: Position) -> String {
        format!("cr1:{}:{}:{}:{}", self.query, self.view, p.span, p.byte)
    }
    pub fn decode(&self, raw: Option<&str>) -> Result<Option<Position>> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        if raw.len() > 256 {
            return Err(AppError::config("invalid code cursor"));
        }
        let parts: Vec<_> = raw.split(':').collect();
        if parts.len() != 5
            || parts[0] != "cr1"
            || parts[1].len() != 64
            || parts[2].len() != 64
            || !parts[1..3]
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(AppError::config("invalid code cursor"));
        }
        let span = parts[3]
            .parse::<usize>()
            .map_err(|_| AppError::config("invalid code cursor position"))?;
        let byte = parts[4]
            .parse::<usize>()
            .map_err(|_| AppError::config("invalid code cursor position"))?;
        if parts[1] != self.query || parts[2] != self.view {
            return Err(AppError::Conflict(
                "CODE_VIEW_CHANGED: restart without cursor".into(),
            ));
        }
        Ok(Some(Position { span, byte }))
    }
}
