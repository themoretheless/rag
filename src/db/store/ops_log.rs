//! Append-only operations log.

use super::*;

/// Recent ops_log rows returned when no id, seq or limit is given.
const DEFAULT_RECENT_OPS: usize = 50;

/// Projection decoded by `row_to_ops_log`.
const OPS_LOG_COLUMNS: &str = "id, COALESCE(seq, 0), CAST(ts AS VARCHAR), op, prefix, \
     COALESCE(message, ''), entity_id, entity_kind, COALESCE(payload_json, '{}'), agent_name";

impl Store {
    /// Next ops_log sequence number (max+1, starting at 1).
    ///
    /// Prefer letting [`Self::append_ops_log`] assign `seq` under the same lock.
    pub fn next_ops_seq(&self) -> Result<i64> {
        let conn = self.lock()?;
        next_ops_seq_locked(&conn)
    }

    /// Append an ops_log row (append-only). Fills `id`, `seq`, `ts`, and
    /// `payload_json` defaults when missing. Requires non-empty `op`.
    pub fn append_ops_log(&self, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
        let conn = self.lock()?;
        append_ops_log_locked(&conn, entry)
    }

    /// Whether the `ops_log` table exists (false on pre-migration / partial DBs).
    pub fn ops_log_exists(&self) -> Result<bool> {
        let conn = self.lock()?;
        Ok(table_exists_locked(&conn, "ops_log"))
    }

    /// Recent ops_log rows, newest first (`seq` DESC, then `ts` DESC).
    ///
    /// Returns an empty list when `ops_log` is missing (no error).
    pub fn list_ops_log(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.list_recent_ops(limit)
    }

    /// Recent ops_log rows, newest first. Alias of [`Self::list_ops_log`].
    ///
    /// Returns an empty list when `ops_log` is missing (no error).
    pub fn list_recent_ops(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        let limit = limit.clamp(1, MAX_OPS_LOG_PAGE) as i64;
        let conn = self.lock()?;
        if !table_exists_locked(&conn, "ops_log") {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT {OPS_LOG_COLUMNS}
            FROM ops_log
            ORDER BY COALESCE(seq, 0) DESC, ts DESC
            LIMIT ?
            "#
        ))?;
        let out = crate::db::rows::query_vec(&mut stmt, params![limit], row_to_ops_log)?;
        Ok(out)
    }

    /// Recent memory-filing ops (ingest / drawer / wiki / diary / checkpoint).
    ///
    /// Used by MCP `memories_filed_away`. Empty when `ops_log` is missing or no
    /// matching rows. Newest first.
    pub fn memories_filed_away(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        let limit = limit.clamp(1, MAX_OPS_LOG_PAGE) as i64;
        let conn = self.lock()?;
        if !table_exists_locked(&conn, "ops_log") {
            return Ok(Vec::new());
        }
        // Filing-related ops: ingest family, drawers, wiki compile, diary, checkpoint.
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT {OPS_LOG_COLUMNS}
            FROM ops_log
            WHERE lower(op) IN (
                'ingest', 'ingest_text', 'ingest_file', 'ingest_raw', 'add_drawer',
                'file_answer', 'wiki_write', 'write_wiki_page', 'update_wiki_page',
                'diary_write', 'checkpoint', 'compile_source'
            )
            OR lower(COALESCE(prefix, '')) IN (
                'ingest', 'drawer', 'wiki', 'diary', 'checkpoint', 'raw'
            )
            ORDER BY COALESCE(seq, 0) DESC, ts DESC
            LIMIT ?
            "#
        ))?;
        let out = crate::db::rows::query_vec(&mut stmt, params![limit], row_to_ops_log)?;
        Ok(out)
    }

    /// Read one ops_log row by primary key `id`.
    pub fn get_ops_log(&self, id: &str) -> Result<Option<OpsLogEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT {OPS_LOG_COLUMNS}
            FROM ops_log WHERE id = ?
            "#
        ))?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_ops_log(row)?)),
            None => Ok(None),
        }
    }

    /// Read one ops_log row by monotonic `seq`.
    pub fn get_ops_log_by_seq(&self, seq: i64) -> Result<Option<OpsLogEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT {OPS_LOG_COLUMNS}
            FROM ops_log WHERE seq = ?
            LIMIT 1
            "#
        ))?;
        let mut rows = stmt.query(params![seq])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_ops_log(row)?)),
            None => Ok(None),
        }
    }

    /// Read ops_log: by `id`, by `seq`, or recent list when both absent.
    ///
    /// When listing, `limit` defaults to 50 and rows are newest-first.
    pub fn read_ops_log(
        &self,
        id: Option<&str>,
        seq: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<OpsLogEntry>> {
        if let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) {
            return Ok(self.get_ops_log(id)?.into_iter().collect());
        }
        if let Some(seq) = seq {
            return Ok(self.get_ops_log_by_seq(seq)?.into_iter().collect());
        }
        self.list_recent_ops(limit.unwrap_or(DEFAULT_RECENT_OPS))
    }
}

pub(super) fn append_ops_log_locked(conn: &Connection, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
    let op = entry.op.trim();
    if op.is_empty() {
        return Err(AppError::config("ops_log op must be non-empty"));
    }
    let mut out = entry.clone();
    out.op = op.to_string();
    if out.id.is_empty() {
        out.id = uuid::Uuid::new_v4().to_string();
    }
    if out.ts.timestamp() == 0 {
        out.ts = Utc::now();
    }
    if out.payload_json.trim().is_empty() {
        out.payload_json = "{}".into();
    }
    if let Some(ref prefix) = out.prefix {
        let prefix = prefix.trim();
        out.prefix = if prefix.is_empty() {
            None
        } else {
            Some(prefix.to_string())
        };
    }
    if out.seq <= 0 {
        out.seq = next_ops_seq_locked(conn)?;
    }
    conn.execute(
        r#"
        INSERT INTO ops_log
          (id, seq, ts, op, prefix, message, entity_id, entity_kind, payload_json, agent_name)
        VALUES
          (?, ?, CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?, ?)
        "#,
        params![
            out.id,
            out.seq,
            format_ts(out.ts),
            out.op,
            out.prefix,
            out.message,
            out.entity_id,
            out.entity_kind,
            out.payload_json,
            out.agent_name,
        ],
    )?;
    Ok(out)
}

pub(super) fn next_ops_seq_locked(conn: &Connection) -> Result<i64> {
    let max: Option<i64> = conn
        .query_row("SELECT MAX(seq) FROM ops_log", [], |r| r.get(0))
        .unwrap_or(None);
    Ok(max.unwrap_or(0) + 1)
}

pub(super) fn row_to_ops_log(row: &duckdb::Row<'_>) -> Result<OpsLogEntry> {
    let ts_raw: String = row.get(2)?;
    Ok(OpsLogEntry {
        id: row.get(0)?,
        seq: row.get(1)?,
        ts: parse_ts(&ts_raw)?,
        op: row.get(3)?,
        prefix: row.get(4)?,
        message: row.get(5)?,
        entity_id: row.get(6)?,
        entity_kind: row.get(7)?,
        payload_json: row.get(8)?,
        agent_name: row.get(9)?,
    })
}
