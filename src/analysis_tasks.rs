//! Durable per-session code-analysis labels, unverified notes and measured call outcomes.
//! These are control records, never code-index facts or document/search inputs.
use crate::{
    db::Store,
    error::{AppError, Result},
};
use duckdb::{params, Connection, OptionalExt};
use rmcp::schemars;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS analysis_tasks (
 session_id VARCHAR PRIMARY KEY, name VARCHAR NOT NULL, generation BIGINT NOT NULL,
 updated_ms BIGINT NOT NULL, expires_ms BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS analysis_notes (
 seq BIGINT PRIMARY KEY, id VARCHAR UNIQUE NOT NULL, session_id VARCHAR NOT NULL,
 task VARCHAR NOT NULL, agent VARCHAR NOT NULL, path VARCHAR NOT NULL,
 tags_json VARCHAR NOT NULL, body VARCHAR NOT NULL, index_rev BIGINT, created_ms BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_analysis_notes_task ON analysis_notes(task,seq);
CREATE TABLE IF NOT EXISTS analysis_calls (
 seq BIGINT PRIMARY KEY, session_id VARCHAR NOT NULL, task VARCHAR NOT NULL,
 tool VARCHAR NOT NULL, started_ms BIGINT NOT NULL, finished_ms BIGINT,
 ok BOOLEAN, evidence_count BIGINT, index_rev BIGINT
);
CREATE INDEX IF NOT EXISTS idx_analysis_calls_task ON analysis_calls(task,seq);
CREATE INDEX IF NOT EXISTS idx_analysis_calls_session ON analysis_calls(session_id,seq);
"#;
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
pub(crate) fn active(conn: &Connection, session: &str, at: i64) -> Result<(String, i64, bool)> {
    let row: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT name,generation,expires_ms FROM analysis_tasks WHERE session_id=?",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.map_or((String::new(), 0, false), |(name, g, expiry)| {
        let expired = expiry <= at;
        (if expired { String::new() } else { name }, g, expired)
    }))
}
fn revision(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?)
}
fn seq(conn: &Connection, table: &str) -> Result<i64> {
    // Only module-owned constants call this helper; no request-provided SQL identifier.
    Ok(conn.query_row(
        &format!("SELECT COALESCE(MAX(seq),0)+1 FROM {table}"),
        [],
        |r| r.get(0),
    )?)
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
    conn: &Connection,
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
        None => active(conn, session, now())?.0,
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
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let (previous, generation, expired) = active(&tx, &session, at)?;
        if q.if_generation.is_some_and(|g| g != generation) {
            return Err(AppError::Conflict(
                "analysis task generation changed".into(),
            ));
        }
        tx.execute("INSERT INTO analysis_tasks VALUES (?,?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET name=excluded.name,generation=excluded.generation,updated_ms=excluded.updated_ms,expires_ms=excluded.expires_ms",params![session,name,generation+1,at,expires])?;
        tx.commit()?;
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
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let task = chosen_task(&tx, &session, q.task.as_deref(), false)?.unwrap();
        let rev = revision(&tx)?;
        let at = now();
        let id = uuid::Uuid::new_v4().to_string();
        let seq = seq(&tx, "analysis_notes")?;
        tx.execute(
            "INSERT INTO analysis_notes VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                seq,
                id,
                session,
                task,
                agent,
                path,
                serde_json::to_string(&tags)?,
                body,
                rev,
                at
            ],
        )?;
        tx.commit()?;
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
        let conn = self.lock()?;
        let task = chosen_task(&conn, &session, q.task.as_deref(), q.all_tasks)?;
        let total:i64=conn.query_row("SELECT COUNT(*) FROM analysis_notes WHERE (? IS NULL OR task=?) AND (? IS NULL OR contains(body,?))",params![task,task,needle,needle],|r|r.get(0))?;
        let mut stmt=conn.prepare("SELECT seq,id,session_id,task,agent,path,tags_json,body,index_rev,created_ms FROM analysis_notes WHERE (? IS NULL OR task=?) AND (? IS NULL OR contains(body,?)) AND (? IS NULL OR seq<?) ORDER BY seq DESC LIMIT ?")?;
        let rows = stmt.query_map(
            params![
                task,
                task,
                needle,
                needle,
                q.before_seq,
                q.before_seq,
                limit as i64 + 1
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                    r.get::<_, i64>(9)?,
                ))
            },
        )?;
        let mut items = vec![];
        for row in rows {
            let (seq, id, session, task, agent, path, tags, body, rev, at) = row?;
            let tags: Vec<String> = serde_json::from_str(&tags)?;
            items.push(json!({"seq":seq,"note_id":id,"session_id":session,"task":task,"agent":agent,"author_identity":"client_asserted","path":path,"tags":tags,"text":body,"index_rev":rev,"created_ms":at,"source":"note","verified_by_index":false}));
        }
        let more = items.len() > limit;
        items.truncate(limit);
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
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let at = now();
        let task = active(&tx, &session, at)?.0;
        let seq = seq(&tx, "analysis_calls")?;
        tx.execute("INSERT INTO analysis_calls(seq,session_id,task,tool,started_ms,index_rev) VALUES (?,?,?,?,?,?)",params![seq,session,task,tool,at,revision(&tx)?])?;
        tx.commit()?;
        Ok(seq)
    }
    pub(crate) fn analysis_call_finish(
        &self,
        seq: i64,
        ok: bool,
        evidence: Option<i64>,
    ) -> Result<()> {
        let conn = self.lock()?;
        if conn.execute("UPDATE analysis_calls SET finished_ms=?,ok=?,evidence_count=? WHERE seq=? AND finished_ms IS NULL",params![now(),ok,if ok{evidence}else{None},seq])?!=1{return Err(AppError::Conflict("analysis call missing or already finished".into()));}
        Ok(())
    }
    pub fn analysis_task_stats(&self, session_id: &str, q: &TaskStats) -> Result<Value> {
        let session = session(session_id)?;
        let format = q.format.as_deref().unwrap_or("json");
        if !matches!(format, "json" | "markdown") {
            return Err(AppError::config("format must be json or markdown"));
        }
        let limit = q.limit.unwrap_or(10).clamp(1, 50);
        let conn = self.lock()?;
        let task = chosen_task(&conn, &session, q.task.as_deref(), q.all_tasks)?;
        let filter = "(? IS NULL OR task=?)";
        let mut stmt=conn.prepare(&format!("SELECT tool,COUNT(*),COUNT(*) FILTER(WHERE ok=true),COUNT(*) FILTER(WHERE ok=false),COUNT(*) FILTER(WHERE finished_ms IS NULL),COUNT(*) FILTER(WHERE evidence_count>0),COUNT(*) FILTER(WHERE ok=true AND evidence_count IS NULL),COALESCE(SUM(evidence_count),0),AVG(finished_ms-started_ms) FROM analysis_calls WHERE {filter} GROUP BY tool ORDER BY tool"))?;
        let rows = stmt.query_map(params![task, task], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, Option<f64>>(8)?,
            ))
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
        for row in rows {
            let (tool, n, s, f, p, h, u, e, ms) = row?;
            calls += n;
            success += s;
            failed += f;
            pending += p;
            nonempty += h;
            unmeasured += u;
            facts += e;
            by_tool.push(json!({"tool":tool,"calls":n,"succeeded":s,"failed":f,"unfinished":p,"nonempty_evidence_calls":h,"successful_unmeasured_calls":u,"returned_evidence_records":e,"avg_elapsed_ms":ms}));
        }
        let notes: i64 = conn.query_row(
            "SELECT COUNT(*) FROM analysis_notes WHERE (? IS NULL OR task=?)",
            params![task, task],
            |r| r.get(0),
        )?;
        let mut stmt=conn.prepare("SELECT seq,session_id,tool,started_ms,finished_ms,ok,evidence_count,index_rev FROM analysis_calls WHERE (? IS NULL OR task=?) ORDER BY seq DESC LIMIT ?")?;
        let rows=stmt.query_map(params![task,task,limit as i64],|r|Ok(json!({"seq":r.get::<_,i64>(0)?,"session_id":r.get::<_,String>(1)?,"tool":r.get::<_,String>(2)?,"started_ms":r.get::<_,i64>(3)?,"finished_ms":r.get::<_,Option<i64>>(4)?,"ok":r.get::<_,Option<bool>>(5)?,"evidence_count":r.get::<_,Option<i64>>(6)?,"index_rev_at_start":r.get::<_,Option<i64>>(7)?})))?;
        let recent = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let measured = success - unmeasured;
        let rate = if measured > 0 {
            Some(nonempty as f64 / measured as f64)
        } else {
            None
        };
        let gate = crate::gate::summary(&conn, task.as_deref(), limit)?;
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
        let s = Store::open(&dir.path().join("tasks.duckdb")).unwrap();
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
        s.lock()
            .unwrap()
            .execute(
                "UPDATE analysis_tasks SET expires_ms=0 WHERE session_id='one'",
                [],
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
