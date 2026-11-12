//! Append-only operations log.

use super::*;
use crate::db::lin_engine::{cell_i64, cell_str};

/// Recent ops_log rows returned when no id, seq or limit is given.
const DEFAULT_RECENT_OPS: usize = 50;

#[cfg(any(test, feature = "testkit"))]
thread_local! {
    #[doc(hidden)]
    pub static TEST_FAIL_OPS_LOG_APPEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Store {
    /// Next ops_log sequence number (max+1, starting at 1).
    pub fn next_ops_seq(&self) -> Result<i64> {
        let r = self.read()?;
        next_ops_seq_locked(&r)
    }

    /// Append an ops_log row (append-only). Fills `id`, `seq`, `ts`, and
    /// `payload_json` defaults when missing. Requires non-empty `op`.
    pub fn append_ops_log(&self, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
        let mut w = self.write()?;
        append_ops_log_locked(&mut w, entry)
    }

    /// Whether the `ops_log` table exists (false on pre-migration / partial DBs).
    pub fn ops_log_exists(&self) -> Result<bool> {
        let r = self.read()?;
        Ok(table_exists_locked(&r, "ops_log"))
    }

    /// Recent ops_log rows, newest first (`seq` DESC, then `ts` DESC).
    pub fn list_ops_log(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.list_recent_ops(limit)
    }

    /// Recent ops_log rows, newest first. Alias of [`Self::list_ops_log`].
    pub fn list_recent_ops(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        let limit = limit.clamp(1, MAX_OPS_LOG_PAGE);
        let r = self.read()?;
        if !table_exists_locked(&r, "ops_log") {
            return Ok(Vec::new());
        }
        let mut out = r.filter_map_rows("ops_log", |row| Ok(Some(rows::ops_log_entry(row)?)))?;
        out.sort_by(|a, b| b.seq.cmp(&a.seq).then_with(|| b.ts.cmp(&a.ts)));
        out.truncate(limit);
        Ok(out)
    }

    /// Recent memory-filing ops (ingest / drawer / wiki / diary / checkpoint).
    pub fn memories_filed_away(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        let limit = limit.clamp(1, MAX_OPS_LOG_PAGE);
        let r = self.read()?;
        if !table_exists_locked(&r, "ops_log") {
            return Ok(Vec::new());
        }
        let mut out = r.filter_map_rows("ops_log", |row| {
            let op_lc = cell_str(row, "op").unwrap_or("").to_ascii_lowercase();
            let prefix_lc = cell_str(row, "prefix").unwrap_or("").to_ascii_lowercase();
            let matches_op = matches!(
                op_lc.as_str(),
                "ingest"
                    | "ingest_text"
                    | "ingest_file"
                    | "ingest_raw"
                    | "add_drawer"
                    | "file_answer"
                    | "wiki_write"
                    | "write_wiki_page"
                    | "update_wiki_page"
                    | "diary_write"
                    | "checkpoint"
                    | "compile_source"
            );
            let matches_prefix = matches!(
                prefix_lc.as_str(),
                "ingest" | "drawer" | "wiki" | "diary" | "checkpoint" | "raw"
            );
            if matches_op || matches_prefix {
                Ok(Some(rows::ops_log_entry(row)?))
            } else {
                Ok(None)
            }
        })?;
        out.sort_by(|a, b| b.seq.cmp(&a.seq).then_with(|| b.ts.cmp(&a.ts)));
        out.truncate(limit);
        Ok(out)
    }

    /// Read one ops_log row by primary key `id`.
    pub fn get_ops_log(&self, id: &str) -> Result<Option<OpsLogEntry>> {
        let r = self.read()?;
        if !table_exists_locked(&r, "ops_log") {
            return Ok(None);
        }
        r.get_by_id("ops_log", id)?
            .as_ref()
            .map(rows::ops_log_entry)
            .transpose()
    }

    /// Read one ops_log row by monotonic `seq`.
    pub fn get_ops_log_by_seq(&self, seq: i64) -> Result<Option<OpsLogEntry>> {
        let r = self.read()?;
        if !table_exists_locked(&r, "ops_log") {
            return Ok(None);
        }
        let mut found = None;
        r.scan_each("ops_log", |row| {
            if cell_i64(row, "seq", 0) == seq {
                found = Some(rows::ops_log_entry(row)?);
                return Ok(false);
            }
            Ok(true)
        })?;
        Ok(found)
    }

    /// Read ops_log: by `id`, by `seq`, or recent list when both absent.
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

pub(super) fn append_ops_log_locked(
    w: &mut impl LinWriter,
    entry: &OpsLogEntry,
) -> Result<OpsLogEntry> {
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
        out.seq = next_ops_seq_locked(w)?;
    }
    #[cfg(any(test, feature = "testkit"))]
    if TEST_FAIL_OPS_LOG_APPEND.with(|f| f.get()) {
        return Err(AppError::db("ops_log table unavailable (injected fault)"));
    }
    w.insert_one(
        "ops_log",
        RecBuilder::new()
            .str("id", &out.id)
            .i64("seq", out.seq)
            .str("ts", format_ts(out.ts))
            .str("op", &out.op)
            .opt_string("prefix", &out.prefix)
            .str("message", &out.message)
            .opt_string("entity_id", &out.entity_id)
            .opt_string("entity_kind", &out.entity_kind)
            .str("payload_json", &out.payload_json)
            .opt_string("agent_name", &out.agent_name)
            .build(),
    )?;
    Ok(out)
}

pub(super) fn next_ops_seq_locked(r: &impl LinReader) -> Result<i64> {
    if !table_exists_locked(r, "ops_log") {
        return Ok(1);
    }
    let mut max_seq = 0i64;
    r.scan_each("ops_log", |row| {
        let s = cell_i64(row, "seq", 0);
        if s > max_seq {
            max_seq = s;
        }
        Ok(true)
    })?;
    Ok(max_seq + 1)
}
