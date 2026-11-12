//! Durable per-session code-analysis labels, unverified notes and measured call outcomes.
//! These are control records, never code-index facts or document/search inputs.
use crate::{
    db::{
        lin_engine::{
            cell_bool_opt, cell_i64, cell_i64_opt, cell_str, cell_string, LinReader, LinWriter,
            RecBuilder,
        },
        Store,
    },
    error::{AppError, Result},
};
use rmcp::schemars;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn bounded(s: &str, max: usize, field: &str) -> Result<String> {
    if s.chars().count() > max || s.contains('\0') {
        return Err(AppError::config(format!(
            "{field} exceeds {max} characters or contains NUL"
        )));
    }
    Ok(s.trim().into())
}
pub(crate) fn session(s: &str) -> Result<String> {
    let s = bounded(s, 200, "session_id")?;
    if s.is_empty() {
        return Err(AppError::config("session_id is empty"));
    }
    Ok(s)
}
pub(crate) fn active(r: &impl LinReader, session: &str, at: i64) -> Result<(String, i64, bool)> {
    let row = r.get_by_id("analysis_tasks", session)?;
    Ok(row.map_or((String::new(), 0, false), |row| {
        let name = cell_string(&row, "name");
        let g = cell_i64(&row, "generation", 0);
        let expiry = cell_i64(&row, "expires_ms", 0);
        let expired = expiry <= at;
        (if expired { String::new() } else { name }, g, expired)
    }))
}
pub(crate) fn latest_code_index_rev(r: &impl LinReader) -> Result<Option<i64>> {
    let mut max_rev: Option<i64> = None;
    r.scan_each("code_index_revisions", |row| {
        if let Some(v) = cell_i64_opt(row, "rev_id") {
            max_rev = Some(max_rev.map_or(v, |prev| prev.max(v)));
        }
        Ok(true)
    })?;
    Ok(max_rev)
}
pub(crate) fn next_seq(r: &impl LinReader, table: &str) -> Result<i64> {
    let mut max_seq = 0i64;
    r.scan_each(table, |row| {
        let v = cell_i64(row, "seq", 0);
        if v > max_seq {
            max_seq = v;
        }
        Ok(true)
    })?;
    Ok(max_seq + 1)
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct TaskSet {
    pub name: Option<String>,
    #[serde(default)]
    pub clear: bool,
    /// Explicit shared label namespace. Omit for this server instance's default session.
    pub session_id: Option<String>,
    /// Prevent replacing a concurrently updated task. Initial generation is zero.
    pub if_generation: Option<i64>,
    /// 1..2592000 seconds; default 86400. Expired labels never attribute new calls.
    pub ttl_seconds: Option<u64>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct Note {
    pub text: String,
    pub task: Option<String>,
    pub session_id: Option<String>,
    pub path: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct Notes {
    pub task: Option<String>,
    pub session_id: Option<String>,
    /// Explicitly include all tasks. Otherwise task defaults to this session's active label.
    #[serde(default)]
    pub all_tasks: bool,
    /// Literal case-sensitive substring, including % and _ as ordinary characters.
    pub query: Option<String>,
    pub before_seq: Option<i64>,
    pub limit: Option<usize>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct TaskStats {
    pub task: Option<String>,
    pub session_id: Option<String>,
    #[serde(default)]
    pub all_tasks: bool,
    /// json (default) or markdown; both retain the structured measured summary.
    pub format: Option<String>,
    pub limit: Option<usize>,
}
fn chosen_task(
    r: &impl LinReader,
    session: &str,
    task: Option<&str>,
    all: bool,
) -> Result<Option<String>> {
    if all && task.is_some() {
        return Err(AppError::config("task and all_tasks cannot be combined"));
    }
    if all {
        return Ok(None);
    }
    Ok(Some(match task {
        Some(t) => bounded(t, 200, "task")?,
        None => active(r, session, now())?.0,
    }))
}
impl Store {
    pub fn analysis_task_set(&self, session_id: &str, q: &TaskSet) -> Result<Value> {
        let session = session(session_id)?;
        let name = bounded(q.name.as_deref().unwrap_or(""), 200, "name")?;
        if q.clear && !name.is_empty() {
            return Err(AppError::config(
                "clear cannot be combined with a nonempty name",
            ));
        }
        let name = if q.clear { String::new() } else { name };
        let ttl = q.ttl_seconds.unwrap_or(86400);
        if !(1..=2592000).contains(&ttl) {
            return Err(AppError::config("ttl_seconds must be 1..2592000"));
        }
        let at = now();
        let expires = at + ttl as i64 * 1000;
        let (previous, generation, expired) = self.with_tx(|tx| {
            let (previous, generation, expired) = active(tx, &session, at)?;
            if q.if_generation.is_some_and(|g| g != generation) {
                return Err(AppError::Conflict(
                    "analysis task generation changed".into(),
                ));
            }
            tx.upsert(
                "analysis_tasks",
                RecBuilder::new()
                    .id(&session)
                    .str("session_id", &session)
                    .str("name", &name)
                    .i64("generation", generation + 1)
                    .i64("updated_ms", at)
                    .i64("expires_ms", expires)
                    .build(),
            )?;
            Ok((previous, generation, expired))
        })?;
        Ok(
            json!({"task":name,"previous":previous,"previous_expired":expired,"generation":generation+1,"session_id":session,"changed":previous!=name,"cleared":name.is_empty(),"expires_ms":expires,"ttl_seconds":ttl,"scope":"database_and_session"}),
        )
    }
    pub fn analysis_note(&self, session_id: &str, agent: &str, q: &Note) -> Result<Value> {
        let session = session(session_id)?;
        let body = bounded(&q.text, 4000, "text")?;
        if body.is_empty() {
            return Err(AppError::config("note text is empty"));
        }
        if q.tags.len() > 10 {
            return Err(AppError::config("at most ten note tags"));
        }
        let mut tags = vec![];
        for tag in &q.tags {
            let tag = bounded(tag, 40, "tag")?;
            if !tag.is_empty() && !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        let path = bounded(q.path.as_deref().unwrap_or(""), 4096, "path")?;
        let agent = bounded(agent, 200, "agent")?;
        let tags_json = serde_json::to_string(&tags)?;
        let (seq, id, task, rev, at) = self.with_tx(|tx| {
            let task = chosen_task(tx, &session, q.task.as_deref(), false)?.unwrap();
            let rev = latest_code_index_rev(tx)?;
            let at = now();
            let id = uuid::Uuid::new_v4().to_string();
            let seq = next_seq(tx, "analysis_notes")?;
            tx.insert_one(
                "analysis_notes",
                RecBuilder::new()
                    .id(seq.to_string())
                    .i64("seq", seq)
                    .str("note_id", &id)
                    .str("session_id", &session)
                    .str("task", &task)
                    .str("agent", &agent)
                    .str("path", &path)
                    .str("tags_json", &tags_json)
                    .str("body", &body)
                    .opt_i64("index_rev", rev)
                    .i64("created_ms", at)
                    .build(),
            )?;
            Ok((seq, id, task, rev, at))
        })?;
        Ok(
            json!({"seq":seq,"note_id":id,"text":body,"task":task,"session_id":session,"agent":agent,"author_identity":"client_asserted","path":path,"tags":tags,"index_rev":rev,"created_ms":at,"source":"note","verified_by_index":false}),
        )
    }
    pub fn analysis_notes(&self, session_id: &str, q: &Notes) -> Result<Value> {
        let session = session(session_id)?;
        let needle = q
            .query
            .as_ref()
            .map(|v| {
                bounded(v, 4000, "query")?;
                Ok::<_, AppError>(v.clone())
            })
            .transpose()?;
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        let r = self.read()?;
        let task = chosen_task(&*r, &session, q.task.as_deref(), q.all_tasks)?;
        let mut total = 0i64;
        let mut matched: Vec<(i64, Value)> = Vec::new();
        r.scan_each("analysis_notes", |row| {
            if let Some(t) = task.as_deref()
                && cell_str(row, "task") != Some(t)
            {
                return Ok(true);
            }
            let body = cell_str(row, "body").unwrap_or_default();
            if let Some(n) = needle.as_deref()
                && !body.contains(n)
            {
                return Ok(true);
            }
            total += 1;
            let seq = cell_i64(row, "seq", 0);
            if q.before_seq.is_some_and(|b| seq >= b) {
                return Ok(true);
            }
            let tags_raw = cell_str(row, "tags_json").unwrap_or("[]");
            let tags: Vec<String> = serde_json::from_str(tags_raw)?;
            let id = cell_string(row, "note_id");
            let row_session = cell_string(row, "session_id");
            let row_task = cell_string(row, "task");
            let agent = cell_string(row, "agent");
            let path = cell_string(row, "path");
            let rev = cell_i64_opt(row, "index_rev");
            let at = cell_i64(row, "created_ms", 0);
            matched.push((
                seq,
                json!({"seq":seq,"note_id":id,"session_id":row_session,"task":row_task,"agent":agent,"author_identity":"client_asserted","path":path,"tags":tags,"text":body,"index_rev":rev,"created_ms":at,"source":"note","verified_by_index":false}),
            ));
            Ok(true)
        })?;
        matched.sort_by_key(|a| std::cmp::Reverse(a.0));
        let more = matched.len() > limit;
        matched.truncate(limit);
        let items: Vec<Value> = matched.into_iter().map(|(_, v)| v).collect();
        let next = if more {
            items.last().and_then(|v| v["seq"].as_i64())
        } else {
            None
        };
        Ok(
            json!({"source":"note","verified_by_index":false,"task":task,"items":items,"total_matching":total,"found":!items.is_empty(),"next_before_seq":next,"truncated":more,"ordering":"seq_desc","scope":"task_labels_in_this_database","notice":"Agent-authored notes are unverified assertions, not code-index facts."}),
        )
    }
    pub(crate) fn analysis_call_start(&self, session_id: &str, tool: &str) -> Result<i64> {
        let session = session(session_id)?;
        if !index_tool(tool) {
            return Err(AppError::config(
                "analysis telemetry only accepts fixed index tools",
            ));
        }
        self.with_tx(|tx| {
            let at = now();
            let task = active(tx, &session, at)?.0;
            let seq = next_seq(tx, "analysis_calls")?;
            let rev = latest_code_index_rev(tx)?;
            tx.insert_one(
                "analysis_calls",
                RecBuilder::new()
                    .id(seq.to_string())
                    .i64("seq", seq)
                    .str("session_id", &session)
                    .str("task", &task)
                    .str("tool", tool)
                    .i64("started_ms", at)
                    .opt_i64("index_rev", rev)
                    .build(),
            )?;
            Ok(seq)
        })
    }
    pub(crate) fn analysis_call_finish(
        &self,
        seq: i64,
        ok: bool,
        evidence: Option<i64>,
    ) -> Result<()> {
        let mut w = self.write()?;
        let id = seq.to_string();
        let Some(existing) = w.get_by_id("analysis_calls", &id)? else {
            return Err(AppError::Conflict(
                "analysis call missing or already finished".into(),
            ));
        };
        if cell_i64_opt(&existing, "finished_ms").is_some() {
            return Err(AppError::Conflict(
                "analysis call missing or already finished".into(),
            ));
        }
        w.update_where(
            "analysis_calls",
            lin::query::pred::eq("id", id.as_str()),
            RecBuilder::new()
                .i64("finished_ms", now())
                .bool("ok", ok)
                .opt_i64("evidence_count", if ok { evidence } else { None })
                .build(),
        )?;
        Ok(())
    }
    pub fn analysis_task_stats(&self, session_id: &str, q: &TaskStats) -> Result<Value> {
        let session = session(session_id)?;
        let format = q.format.as_deref().unwrap_or("json");
        if !matches!(format, "json" | "markdown") {
            return Err(AppError::config("format must be json or markdown"));
        }
        let limit = q.limit.unwrap_or(10).clamp(1, 50);
        let r = self.read()?;
        let task = chosen_task(&*r, &session, q.task.as_deref(), q.all_tasks)?;

        #[derive(Default)]
        struct ToolAcc {
            calls: i64,
            succeeded: i64,
            failed: i64,
            unfinished: i64,
            nonempty: i64,
            unmeasured: i64,
            evidence_sum: i64,
            elapsed_sum: f64,
            elapsed_n: i64,
        }

        let mut grouped: BTreeMap<String, ToolAcc> = BTreeMap::new();
        let mut recent_rows: Vec<(i64, Value)> = Vec::new();
        r.scan_each("analysis_calls", |row| {
            if let Some(t) = task.as_deref()
                && cell_str(row, "task") != Some(t)
            {
                return Ok(true);
            }
            let seq = cell_i64(row, "seq", 0);
            let row_session = cell_string(row, "session_id");
            let tool = cell_string(row, "tool");
            let started_ms = cell_i64(row, "started_ms", 0);
            let finished_ms = cell_i64_opt(row, "finished_ms");
            let ok = cell_bool_opt(row, "ok");
            let evidence_count = cell_i64_opt(row, "evidence_count");
            let index_rev = cell_i64_opt(row, "index_rev");

            let acc = grouped.entry(tool.clone()).or_default();
            acc.calls += 1;
            if ok == Some(true) {
                acc.succeeded += 1;
                if evidence_count.is_none() {
                    acc.unmeasured += 1;
                }
            } else if ok == Some(false) {
                acc.failed += 1;
            }
            if let Some(fin) = finished_ms {
                acc.elapsed_sum += (fin - started_ms) as f64;
                acc.elapsed_n += 1;
            } else {
                acc.unfinished += 1;
            }
            if let Some(ev) = evidence_count {
                if ev > 0 {
                    acc.nonempty += 1;
                }
                acc.evidence_sum += ev;
            }

            recent_rows.push((
                seq,
                json!({"seq":seq,"session_id":row_session,"tool":tool,"started_ms":started_ms,"finished_ms":finished_ms,"ok":ok,"evidence_count":evidence_count,"index_rev_at_start":index_rev}),
            ));
            Ok(true)
        })?;

        let (
            mut calls,
            mut success,
            mut failed,
            mut pending,
            mut nonempty,
            mut unmeasured,
            mut facts,
        ) = (0, 0, 0, 0, 0, 0, 0);
        let mut by_tool = vec![];
        for (tool, acc) in grouped {
            calls += acc.calls;
            success += acc.succeeded;
            failed += acc.failed;
            pending += acc.unfinished;
            nonempty += acc.nonempty;
            unmeasured += acc.unmeasured;
            facts += acc.evidence_sum;
            let avg_ms = (acc.elapsed_n > 0).then_some(acc.elapsed_sum / acc.elapsed_n as f64);
            by_tool.push(json!({"tool":tool,"calls":acc.calls,"succeeded":acc.succeeded,"failed":acc.failed,"unfinished":acc.unfinished,"nonempty_evidence_calls":acc.nonempty,"successful_unmeasured_calls":acc.unmeasured,"returned_evidence_records":acc.evidence_sum,"avg_elapsed_ms":avg_ms}));
        }

        let mut notes = 0i64;
        r.scan_each("analysis_notes", |row| {
            if task.as_deref().is_none_or(|t| cell_str(row, "task") == Some(t)) {
                notes += 1;
            }
            Ok(true)
        })?;

        recent_rows.sort_by_key(|a| std::cmp::Reverse(a.0));
        recent_rows.truncate(limit);
        let recent: Vec<Value> = recent_rows.into_iter().map(|(_, v)| v).collect();

        let measured = success - unmeasured;
        let rate = if measured > 0 {
            Some(nonempty as f64 / measured as f64)
        } else {
            None
        };
        let gate = crate::gate::summary(&*r, task.as_deref(), limit)?;
        let label = task.as_deref().unwrap_or("all tasks");
        let markdown = if format == "markdown" {
            Some(format!("Task: {}\n\nCalls: {calls}; succeeded: {success}; failed: {failed}; unfinished: {pending}.\nMeasured nonempty evidence calls: {nonempty}; returned evidence records: {facts}; notes: {notes}.\nSaved calls and correctness are not measured.",label.replace(['\n','\r']," ")))
        } else {
            None
        };
        Ok(
            json!({"task":task,"scope":"task_labels_in_this_database","calls":calls,"succeeded":success,"failed_calls":failed,"unfinished":pending,"nonempty_evidence_calls":nonempty,"successful_unmeasured_calls":unmeasured,"returned_evidence_records":facts,"measured_nonempty_rate":rate,"notes_total":notes,"by_tool":by_tool,"recent":recent,"recent_truncated":calls>limit as i64,"markdown":markdown,"sources":["analysis_calls","analysis_notes","analysis_gate_decisions"],"not_measured":["saved_tool_calls","runtime_correctness","evidence_quality","live_source_staleness","gate_effectiveness"],"gate":gate}),
        )
    }
}
/// Fixed exact whitelist. A caller-provided tool prefix is not proof the index was used.
pub(crate) fn index_tool(name: &str) -> bool {
    matches!(
        name,
        "search"
            | "search_multi"
            | "duplicates"
            | "symbol"
            | "references"
            | "callers"
            | "callees"
            | "code"
            | "repo_map"
            | "impact"
            | "trace_path"
            | "affected_tests"
            | "inheritance"
            | "entrypoints"
            | "routes"
            | "dead_code"
            | "slice"
            | "data_slice"
            | "schema"
            | "table_usage"
            | "write_check"
            | "class_usage"
            | "policy_trace"
            | "freshness"
            | "index_status"
            | "catalog"
            | "diff"
            | "live_read"
    )
}
/// Count only known output containers. This measures returned records, not truthful/useful facts.
pub(crate) fn evidence_count(value: &Value) -> Option<i64> {
    if value.get("found") == Some(&Value::Bool(false)) {
        return Some(0);
    }
    if let Some(a) = value.as_array() {
        return Some(a.len() as i64);
    }
    for key in [
        "items",
        "hits",
        "symbols",
        "references",
        "calls",
        "edges",
        "tables",
        "entrypoints",
        "routes",
        "paths",
    ] {
        if let Some(a) = value.get(key).and_then(Value::as_array) {
            return Some(a.len() as i64);
        }
    }
    if value.get("found") == Some(&Value::Bool(true)) {
        return Some(1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("tasks.lin")).unwrap();
        (dir, s)
    }
    fn label(s: &Store, session: &str, name: &str) -> Value {
        s.analysis_task_set(
            session,
            &TaskSet {
                name: Some(name.into()),
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn labels_are_session_isolated_expire_clear_and_cas_never_overwrites() {
        let (_d, s) = store();
        assert_eq!(label(&s, "one", "alpha")["generation"], 1);
        label(&s, "two", "beta");
        let conflict = s.analysis_task_set(
            "one",
            &TaskSet {
                name: Some("bad".into()),
                if_generation: Some(0),
                ..Default::default()
            },
        );
        assert!(conflict.is_err());
        assert_eq!(
            s.analysis_task_stats("one", &Default::default()).unwrap()["task"],
            "alpha"
        );
        assert_eq!(
            s.analysis_task_stats("two", &Default::default()).unwrap()["task"],
            "beta"
        );
        s.write()
            .unwrap()
            .update_where(
                "analysis_tasks",
                lin::query::pred::eq("id", "one"),
                RecBuilder::new().i64("expires_ms", 0).build(),
            )
            .unwrap();
        let r = s
            .analysis_task_set(
                "one",
                &TaskSet {
                    name: Some("fresh".into()),
                    if_generation: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(r["previous_expired"], true);
        assert_eq!(r["generation"], 2);
        s.analysis_task_set(
            "one",
            &TaskSet {
                clear: true,
                if_generation: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            s.analysis_task_stats("one", &Default::default()).unwrap()["task"],
            ""
        );
        assert!(s
            .analysis_task_set(
                "x",
                &TaskSet {
                    ttl_seconds: Some(0),
                    ..Default::default()
                }
            )
            .is_err());
    }
    #[test]
    fn notes_are_append_only_unverified_literal_filtered_and_cursor_stable() {
        let (dir, s) = store();
        label(&s, "one", "alpha");
        let q = Note {
            text: "  100%_ exact  ".into(),
            tags: vec!["a,b".into(), "a,b".into(), "c".into()],
            ..Default::default()
        };
        let first = s.analysis_note("one", "test-client", &q).unwrap();
        let second = s.analysis_note("one", "test-client", &q).unwrap();
        assert_ne!(first["note_id"], second["note_id"]);
        assert_eq!(first["tags"], json!(["a,b", "c"]));
        assert_eq!(first["source"], "note");
        assert_eq!(first["verified_by_index"], false);
        s.analysis_note(
            "one",
            "test-client",
            &Note {
                text: "100xx exact".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let q = Notes {
            query: Some("%_".into()),
            limit: Some(1),
            ..Default::default()
        };
        let page = s.analysis_notes("one", &q).unwrap();
        assert_eq!(page["total_matching"], 2);
        assert_eq!(page["truncated"], true);
        let cursor = page["next_before_seq"].as_i64().unwrap();
        s.analysis_note(
            "one",
            "test-client",
            &Note {
                text: "new %_".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let page = s
            .analysis_notes(
                "one",
                &Notes {
                    before_seq: Some(cursor),
                    ..q
                },
            )
            .unwrap();
        assert_eq!(page["items"][0]["note_id"], first["note_id"]);
        assert_eq!(page["truncated"], false);
        assert!(s.list_documents().unwrap().is_empty());
        s.index_code().unwrap();
        assert!(s.code_snapshot().unwrap().symbols.is_empty());
        let path = s.path().to_owned();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(
            s.analysis_notes("one", &Default::default()).unwrap()["total_matching"],
            4
        );
        drop(dir);
    }
    #[test]
    fn notes_reject_silent_truncation_and_invalid_filters() {
        let (_d, s) = store();
        assert!(s
            .analysis_note(
                "x",
                "a",
                &Note {
                    text: "x".repeat(4001),
                    ..Default::default()
                }
            )
            .is_err());
        assert!(s
            .analysis_note(
                "x",
                "a",
                &Note {
                    text: " ".into(),
                    ..Default::default()
                }
            )
            .is_err());
        assert!(s
            .analysis_note(
                "x",
                "a",
                &Note {
                    text: "ok".into(),
                    tags: vec!["a".into(); 11],
                    ..Default::default()
                }
            )
            .is_err());
        assert!(s
            .analysis_notes(
                "x",
                &Notes {
                    task: Some("x".into()),
                    all_tasks: true,
                    ..Default::default()
                }
            )
            .is_err());
        assert!(s
            .analysis_task_stats(
                "x",
                &TaskStats {
                    format: Some("html".into()),
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(
            s.analysis_notes("x", &Default::default()).unwrap()["total_matching"],
            0
        );
    }
    #[test]
    fn stats_capture_task_at_start_and_keep_failure_pending_and_unknown_distinct() {
        let (_d, s) = store();
        label(&s, "one", "alpha");
        let first = s.analysis_call_start("one", "symbol").unwrap();
        label(&s, "one", "beta");
        s.analysis_call_finish(first, true, Some(2)).unwrap();
        let failed = s.analysis_call_start("one", "symbol").unwrap();
        s.analysis_call_finish(failed, false, Some(100)).unwrap();
        let unknown = s.analysis_call_start("one", "code").unwrap();
        s.analysis_call_finish(unknown, true, None).unwrap();
        s.analysis_call_start("one", "symbol").unwrap();
        assert!(s.analysis_call_finish(first, true, Some(5)).is_err());
        let alpha = s
            .analysis_task_stats(
                "one",
                &TaskStats {
                    task: Some("alpha".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(alpha["calls"], 1);
        assert_eq!(alpha["returned_evidence_records"], 2);
        let beta = s.analysis_task_stats("one", &Default::default()).unwrap();
        assert_eq!(beta["calls"], 3);
        assert_eq!(beta["failed_calls"], 1);
        assert_eq!(beta["unfinished"], 1);
        assert_eq!(beta["successful_unmeasured_calls"], 1);
        assert!(beta["measured_nonempty_rate"].is_null());
        assert_eq!(beta["returned_evidence_records"], 0);
        let zero = s
            .analysis_task_stats(
                "one",
                &TaskStats {
                    task: Some("absent".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(zero["calls"], 0);
        assert!(zero["measured_nonempty_rate"].is_null());
        assert!(!index_tool("ragraf_fake"));
        assert!(s.analysis_call_start("one", "ragraf_fake").is_err());
    }
}
