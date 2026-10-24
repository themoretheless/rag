//! A bounded index-first usage gate, shared by MCP and the opt-in hook executable.
//! This is not a sandbox, an authorization engine or a command executor.
use crate::{
    analysis_tasks,
    db::Store,
    error::{AppError, Result},
    util::content_hash,
};
use duckdb::{params, Connection, OptionalExt};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};
const WINDOW_MS: i64 = 900_000;
const MAX_FILES: usize = 256;
const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS analysis_gate_decisions (
 seq BIGINT PRIMARY KEY, id VARCHAR UNIQUE NOT NULL, session_id VARCHAR NOT NULL,
 task VARCHAR NOT NULL, index_rev BIGINT, created_ms BIGINT NOT NULL,
 mode VARCHAR NOT NULL, kind VARCHAR NOT NULL, verdict VARCHAR NOT NULL,
 rule VARCHAR NOT NULL, facts_json VARCHAR NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_analysis_gate_task ON analysis_gate_decisions(task,seq);
CREATE INDEX IF NOT EXISTS idx_analysis_gate_session ON analysis_gate_decisions(session_id,seq);
"#;
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GateQuery {
    /// Hook object or JSON string: tool_name, tool_input, cwd, session_id, hook_event_name.
    pub payload: Value,
    /// warn (default), ask, deny, off. No configuration or client hook is installed.
    pub mode: Option<String>,
}
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Payload {
    #[serde(default, alias = "tool")]
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Value,
    #[serde(default)]
    pub cwd: String,
    pub session_id: Option<String>,
    #[serde(default, alias = "event")]
    pub hook_event_name: String,
}
pub fn parse_payload(value: &Value) -> Result<Payload> {
    if serde_json::to_vec(value)?.len() > 65536 {
        return Err(AppError::config("gate payload exceeds 64 KiB"));
    }
    let data = if let Some(s) = value.as_str() {
        serde_json::from_str(s)?
    } else {
        value.clone()
    };
    if !data.is_object() {
        return Err(AppError::config("gate payload must be a nonempty object"));
    }
    let p: Payload = serde_json::from_value(data)?;
    if p.tool_name.is_empty() && p.hook_event_name != "SessionStart" {
        return Err(AppError::config("gate requires tool_name"));
    }
    if p.tool_name.len() > 200 || p.cwd.len() > 4096 {
        return Err(AppError::config("gate tool/cwd exceeds limit"));
    }
    Ok(p)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Warn,
    Ask,
    Deny,
    Off,
}
impl Mode {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "warn" => Ok(Self::Warn),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            "off" => Ok(Self::Off),
            _ => Err(AppError::config("gate mode must be warn, ask, deny or off")),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Ask => "ask",
            Self::Deny => "deny",
            Self::Off => "off",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub verdict: String,
    pub rule: String,
    pub reason: String,
    pub context: String,
    pub facts: Value,
}
impl Decision {
    /// Claude-compatible PreToolUse response; deny is exit 2, ask is explicit JSON.
    pub fn hook(&self) -> Value {
        let (stdout,exit,stderr)=match self.verdict.as_str(){
            "deny"=>(String::new(),2,self.reason.clone()),
            "ask"=>(json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":self.reason,"additionalContext":self.context}}).to_string(),0,String::new()),
            _=>(if self.context.is_empty(){String::new()}else{json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":self.context}}).to_string()},0,String::new()),
        };
        json!({"stdout":stdout,"exit_code":exit,"stderr":stderr})
    }
}
#[derive(Default)]
struct Classification {
    kind: &'static str,
    is_code: bool,
    uncertain: bool,
}
fn children(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}
fn literal(s: &str) -> Option<&str> {
    let s = s.trim();
    if let Some(q) = s.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        if !s.ends_with(q) || s.len() < 2 {
            return None;
        }
        let inner = &s[1..s.len() - 1];
        if inner.contains(['\\', '$', '`']) {
            None
        } else {
            Some(inner)
        }
    } else if s.contains(['\\', '$', '`', '\'', '"']) {
        None
    } else {
        Some(s)
    }
}
fn shell_commands(n: Node, source: &str, depth: usize, search: &mut bool, unknown: &mut bool) {
    if depth >= 64 {
        *unknown = true;
        return;
    }
    if n.kind() == "command" {
        let Some(name) = n.child_by_field_name("name") else {
            *unknown = true;
            return;
        };
        let Some(name) = literal(&source[name.byte_range()]) else {
            *unknown = true;
            return;
        };
        let name = Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(name)
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "rg" | "grep" | "egrep" | "fgrep" | "ripgrep" | "findstr" | "select-string" | "find"
        ) {
            *search = true;
        }
        let args: Vec<_> = children(n)
            .into_iter()
            .filter(|c| {
                n.child_by_field_name("name")
                    .is_none_or(|x| x.id() != c.id())
            })
            .collect();
        if name == "ls"
            && args.iter().any(|a| {
                literal(&source[a.byte_range()])
                    .is_some_and(|s| s == "--recursive" || (s.starts_with('-') && s.contains('R')))
            })
        {
            *search = true;
        }
        if name == "get-childitem"
            && args.iter().any(|a| {
                literal(&source[a.byte_range()]).is_some_and(|s| {
                    let s = s.to_ascii_lowercase();
                    s.starts_with("-recurse") || s.starts_with("-filter")
                })
            })
        {
            *search = true;
        }
        if matches!(name.as_str(), "gci" | "sls") {
            *unknown = true;
        }
        if name == "git" {
            let mut i = 0;
            while i < args.len() {
                let Some(arg) = literal(&source[args[i].byte_range()]) else {
                    *unknown = true;
                    break;
                };
                if matches!(arg, "-C" | "-c" | "--git-dir" | "--work-tree") {
                    i += 2;
                    continue;
                }
                if arg.starts_with('-') {
                    i += 1;
                    continue;
                }
                if arg == "grep" {
                    *search = true;
                }
                break;
            }
        }
        if matches!(name.as_str(), "bash" | "sh" | "zsh") {
            if let Some(i) = args.iter().position(|a| {
                literal(&source[a.byte_range()]).is_some_and(|s| s == "-c" || s == "-lc")
            }) {
                if let Some(script) = args
                    .get(i + 1)
                    .and_then(|a| literal(&source[a.byte_range()]))
                {
                    let (a, b) = shell(script, depth + 1);
                    *search |= a;
                    *unknown |= b;
                } else {
                    *unknown = true;
                }
            }
        }
        if matches!(name.as_str(), "eval" | "powershell" | "pwsh" | "cmd") {
            *unknown = true;
        }
        if matches!(
            name.as_str(),
            "env" | "command" | "sudo" | "doas" | "nice" | "timeout"
        ) {
            if let Some(a) = args.iter().find_map(|a| {
                literal(&source[a.byte_range()]).filter(|s| !s.starts_with('-') && !s.contains('='))
            }) {
                if matches!(a, "rg" | "grep" | "findstr" | "find") {
                    *search = true;
                } else {
                    *unknown = true;
                }
            }
        }
    }
    for child in children(n) {
        shell_commands(child, source, depth + 1, search, unknown);
    }
}
fn shell(source: &str, depth: usize) -> (bool, bool) {
    if depth >= 8 {
        return (false, true);
    }
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return (false, true);
    }
    let Some(tree) = parser.parse(source, None) else {
        return (false, true);
    };
    let mut search = false;
    let mut unknown = tree.root_node().has_error();
    shell_commands(tree.root_node(), source, depth, &mut search, &mut unknown);
    (search, unknown)
}
fn classify(p: &Payload) -> Result<Classification> {
    let name = p.tool_name.to_ascii_lowercase();
    let canonical = name
        .strip_prefix("mcp__rag_mcp__")
        .or_else(|| name.strip_prefix("ragraf_"))
        .unwrap_or(&name);
    if analysis_tasks::index_tool(canonical) {
        return Ok(Classification {
            kind: "index",
            ..Default::default()
        });
    }
    if matches!(
        name.as_str(),
        "grep" | "ripgrep" | "search_files" | "glob" | "glob_files"
    ) {
        return Ok(Classification {
            kind: "file_search",
            ..Default::default()
        });
    }
    let input = if let Some(s) = p.tool_input.as_str() {
        serde_json::from_str::<Value>(s)?
    } else {
        p.tool_input.clone()
    };
    let command = input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(Value::as_str);
    if let Some(command) = command {
        if command.len() > 16384 {
            return Err(AppError::config("gate command exceeds 16 KiB"));
        }
        let (search, uncertain) = shell(command, 0);
        return Ok(Classification {
            kind: if search {
                "file_search"
            } else if uncertain {
                "unknown_shell"
            } else {
                "other"
            },
            uncertain,
            ..Default::default()
        });
    }
    if matches!(name.as_str(), "read" | "read_file" | "view" | "open") {
        let path = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .or_else(|| input.get("file"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let ext = Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        return Ok(Classification {
            kind: "read",
            is_code: matches!(
                ext.as_str(),
                "rs" | "py"
                    | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "php"
                    | "phtml"
                    | "c"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "cs"
                    | "go"
                    | "java"
                    | "rb"
                    | "lua"
                    | "pl"
                    | "pm"
                    | "ps1"
                    | "sh"
                    | "bash"
                    | "css"
                    | "html"
                    | "htm"
                    | "sql"
                    | "vue"
                    | "svelte"
            ),
            ..Default::default()
        });
    }
    Ok(Classification {
        kind: "other",
        ..Default::default()
    })
}
struct Facts {
    corpus: Option<String>,
    rev: Option<i64>,
    fresh: Option<bool>,
    checked: usize,
    bytes: usize,
    reasons: Vec<&'static str>,
}
fn stored_changes(conn: &Connection, rev: i64, root: &str) -> Result<i64> {
    let prefix = format!(
        "{}{}",
        root.trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );
    let changed:i64=conn.query_row("SELECT COUNT(*) FROM code_index_revision_files f LEFT JOIN documents d ON d.id=f.document_id WHERE f.index_revision=? AND (f.path=? OR starts_with(f.path,?)) AND (d.id IS NULL OR COALESCE(d.status,'active') IN ('archived','tombstone','consolidated') OR COALESCE(d.revision,1)<>f.document_revision OR COALESCE(d.source_file,d.uri)<>f.path)",params![rev,root,prefix],|r|r.get(0))?;
    let new:i64=conn.query_row("SELECT COUNT(*) FROM documents d WHERE COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') AND (COALESCE(d.source_file,d.uri)=? OR starts_with(COALESCE(d.source_file,d.uri),?)) AND NOT EXISTS(SELECT 1 FROM code_index_revision_files f WHERE f.index_revision=? AND f.document_id=d.id)",params![root,prefix,rev],|r|r.get(0))?;
    Ok(changed + new)
}
fn freshness(store: &Store, cwd: &str, roots: &[PathBuf]) -> Result<Facts> {
    let mut f = Facts {
        corpus: None,
        rev: None,
        fresh: None,
        checked: 0,
        bytes: 0,
        reasons: vec![],
    };
    let path = Path::new(cwd);
    if !path.is_absolute() || crate::util::check_path_allowlist(path, roots).is_err() {
        f.reasons.push("CWD_OUTSIDE_ALLOWED_ROOTS_OR_NOT_ABSOLUTE");
        return Ok(f);
    }
    let Ok(cwd) = path.canonicalize() else {
        f.reasons.push("CWD_UNAVAILABLE");
        return Ok(f);
    };
    let (files, changed, format_matches) = {
        let conn = store.lock()?;
        let mut stmt=conn.prepare("SELECT DISTINCT canonical_root FROM source_manifest ORDER BY length(canonical_root) DESC,canonical_root")?;
        let all = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        f.corpus = all.into_iter().find(|r| cwd.starts_with(Path::new(r)));
        let Some(root) = f.corpus.as_ref() else {
            return Ok(f);
        };
        let latest: Option<(i64, String)> = conn
            .query_row(
                "SELECT id,graph_json FROM code_index_revisions ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((rev, graph)) = latest else {
            f.fresh = Some(false);
            f.reasons.push("INDEX_MISSING");
            return Ok(f);
        };
        f.rev = Some(rev);
        let format_matches = serde_json::from_str::<Value>(&graph)?
            .get("format_version")
            .and_then(Value::as_u64)
            == Some(crate::code_index::INDEX_FORMAT_VERSION as u64);
        let prefix = format!(
            "{}{}",
            root.trim_end_matches(std::path::MAIN_SEPARATOR),
            std::path::MAIN_SEPARATOR
        );
        let mut stmt=conn.prepare("SELECT f.path,f.content_hash FROM code_index_revision_files f WHERE f.index_revision=? AND (f.path=? OR starts_with(f.path,?)) ORDER BY f.path LIMIT ?")?;
        let files = stmt
            .query_map(params![rev, root, prefix, MAX_FILES as i64 + 1], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let changed = stored_changes(&conn, rev, root)?;
        (files, changed, format_matches)
    };
    if !format_matches || changed > 0 {
        f.fresh = Some(false);
        f.reasons.push("STORED_INDEX_INPUTS_CHANGED");
        return Ok(f);
    }
    if files.is_empty() {
        f.reasons.push("NO_INDEXED_FILES_IN_ROOT");
        return Ok(f);
    }
    let truncated = files.len() > MAX_FILES;
    for (path, hash) in files.into_iter().take(MAX_FILES) {
        if f.bytes >= MAX_BYTES {
            f.reasons.push("LIVE_BYTE_BUDGET_EXCEEDED");
            break;
        }
        match crate::code_index::drift::read_live_source(&path, roots, MAX_BYTES - f.bytes) {
            Ok(Some(s)) => {
                f.checked += 1;
                f.bytes += s.bytes;
                if content_hash(&s.text) != hash {
                    f.fresh = Some(false);
                    f.reasons.push("INDEXED_FILE_CHANGED");
                    return Ok(f);
                }
            }
            Ok(None) => {
                f.fresh = Some(false);
                f.reasons.push("INDEXED_FILE_MISSING");
                return Ok(f);
            }
            Err(_) => {
                f.reasons.push("LIVE_SOURCE_UNAVAILABLE_OR_BUDGET_EXCEEDED");
                break;
            }
        }
    }
    if truncated {
        f.reasons.push("LIVE_FILE_BUDGET_EXCEEDED");
    }
    if f.reasons.is_empty() {
        f.fresh = Some(true);
    }
    // Do not certify a revision that changed during the unlocked bounded live reads.
    let current: Option<i64> =
        store
            .lock()?
            .query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?;
    if current != f.rev {
        f.fresh = None;
        f.reasons.push("INDEX_REVISION_CHANGED_DURING_CHECK");
    }
    Ok(f)
}
fn decide(c: &Classification, f: &Facts, mode: Mode, used: bool, first: bool) -> Decision {
    let mut d = Decision {
        verdict: "allow".into(),
        rule: String::new(),
        reason: String::new(),
        context: String::new(),
        facts: Value::Null,
    };
    let issue = if f.fresh == Some(false) {
        "stale_index"
    } else if f.fresh.is_none() {
        "freshness_unknown"
    } else if !used {
        "index_unused"
    } else {
        "index_used"
    };
    let rule = if mode == Mode::Off {
        d.verdict = "off".into();
        "gate_off"
    } else if c.kind == "session_start" {
        "session_start"
    } else if c.kind == "index" {
        "index_call_not_completion"
    } else if c.kind == "other" {
        "not_relevant"
    } else if f.corpus.is_none() {
        "no_registered_corpus"
    } else if c.kind == "read" {
        if c.is_code && !used {
            "read_hint"
        } else {
            "read_ok"
        }
    } else {
        issue
    };
    d.rule = rule.into();
    d.reason=match rule{"stale_index"=>"The indexed inputs changed; synchronize sources before relying on this index.","freshness_unknown"=>"Freshness could not be verified within the allowed-source budgets.","index_unused"=>"No successful nonempty index query at this revision was recorded for this task/session in the last 15 minutes.","read_hint"=>"Reading code is allowed; symbol/search_multi/impact may provide useful context first.","index_call_not_completion"=>"Index calls are allowed; a declared call does not count as completed index use.",_=>"This index-usage gate has no objection to this call."}.into();
    if matches!(rule, "stale_index" | "freshness_unknown" | "index_unused") {
        d.verdict = match mode {
            Mode::Warn => "allow",
            Mode::Ask => {
                if first {
                    "ask"
                } else {
                    "deny"
                }
            }
            Mode::Deny => "deny",
            Mode::Off => "off",
        }
        .into();
        d.context="Use sync_sources/freshness as appropriate, then a scoped symbol/search_multi query. This gate does not execute or rewrite the proposed command.".into();
    } else if rule == "read_hint" {
        d.context = "Consider symbol/search_multi and impact for code navigation.".into();
    }
    d
}
impl Store {
    pub fn analysis_gate(
        &self,
        session_id: &str,
        p: &Payload,
        mode: Mode,
        roots: &[PathBuf],
    ) -> Result<Value> {
        let session = analysis_tasks::session(session_id)?;
        let at = chrono::Utc::now().timestamp_millis();
        let task = analysis_tasks::active(&*self.lock()?, &session, at)?.0;
        let c = if mode == Mode::Off {
            Classification {
                kind: "off",
                ..Default::default()
            }
        } else if p.hook_event_name == "SessionStart" {
            Classification {
                kind: "session_start",
                ..Default::default()
            }
        } else {
            classify(p)?
        };
        let mut f =
            if mode != Mode::Off && matches!(c.kind, "file_search" | "read" | "unknown_shell") {
                freshness(self, &p.cwd, roots)?
            } else {
                Facts {
                    corpus: None,
                    rev: None,
                    fresh: None,
                    checked: 0,
                    bytes: 0,
                    reasons: vec![],
                }
            };
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let rev: Option<i64> =
            tx.query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?;
        if f.rev.is_some() && f.rev != rev {
            f.fresh = None;
            f.reasons.push("INDEX_REVISION_CHANGED_BEFORE_DECISION");
        }
        if f.rev == rev {
            if let (Some(rev), Some(root)) = (rev, f.corpus.as_deref()) {
                if stored_changes(&tx, rev, root)? > 0 {
                    f.fresh = Some(false);
                    f.reasons.push("STORED_INPUTS_CHANGED_BEFORE_DECISION");
                }
            }
        }
        let epoch:Option<(i64,String)>=tx.query_row("SELECT seq,facts_json FROM analysis_gate_decisions WHERE session_id=? AND kind='session_start' ORDER BY seq DESC LIMIT 1",[&session],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let epoch_gate = epoch.as_ref().map_or(0, |e| e.0);
        let epoch_call = epoch
            .as_ref()
            .map(|e| serde_json::from_str::<Value>(&e.1))
            .transpose()?
            .and_then(|v| v["index_use_barrier_seq"].as_i64())
            .unwrap_or(0);
        let used:i64=tx.query_row("SELECT COUNT(*) FROM analysis_calls WHERE session_id=? AND task=? AND ok=true AND finished_ms>=? AND started_ms<=? AND index_rev IS NOT DISTINCT FROM ? AND evidence_count>0 AND seq>?",params![session,task,at-WINDOW_MS,at,rev,epoch_call],|r|r.get(0))?;
        let prev:i64=tx.query_row("SELECT COUNT(*) FROM analysis_gate_decisions WHERE session_id=? AND task=? AND index_rev IS NOT DISTINCT FROM ? AND created_ms>=? AND mode='ask' AND verdict IN ('ask','deny') AND seq>?",params![session,task,rev,at-WINDOW_MS,epoch_gate],|r|r.get(0))?;
        let mut d = decide(&c, &f, mode, used > 0, prev == 0);
        d.facts = json!({"kind":c.kind,"classification_uncertain":c.uncertain,"corpus_root":f.corpus,"fresh":f.fresh,"index_rev":f.rev.or(rev),"freshness_scope":"indexed_manifest_inputs_only_no_new_file_discovery","checked_files":f.checked,"checked_bytes":f.bytes,"checked_bytes_basis":"original_file_bytes","checksum_scope":"decoded_source_utf8","freshness_reasons":f.reasons,"index_used":used>0,"index_use_scope":"same_database_task_session_revision_at_call_start_successful_nonempty","first_offence":prev==0,"mode":mode,"budgets":{"files":MAX_FILES,"total_bytes":MAX_BYTES,"per_file_bytes":4*1024*1024}});
        if c.kind == "session_start" {
            let barrier: i64 =
                tx.query_row("SELECT COALESCE(MAX(seq),0) FROM analysis_calls", [], |r| {
                    r.get(0)
                })?;
            d.facts["index_use_barrier_seq"] = json!(barrier);
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM analysis_gate_decisions",
            [],
            |r| r.get(0),
        )?;
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO analysis_gate_decisions VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                seq,
                id,
                session,
                task,
                rev,
                at,
                mode.as_str(),
                c.kind,
                d.verdict,
                d.rule,
                serde_json::to_string(&d.facts)?
            ],
        )?;
        let counters = summary(&tx, Some(&task), 5)?;
        tx.commit()?;
        Ok(
            json!({"decision_id":id,"seq":seq,"session_id":session,"task":task,"index_rev":rev,"verdict":d.verdict,"rule":d.rule,"reason":d.reason,"context":d.context,"facts":d.facts,"hook":d.hook(),"counters":counters,"command_executed":false,"client_hook_installed":false,"not_checked":["command_safety","runtime_authorization","unindexed_new_files","whether_prior_queries_covered_target_project"]}),
        )
    }
}
pub(crate) fn summary(conn: &Connection, task: Option<&str>, limit: usize) -> Result<Value> {
    let mut stmt=conn.prepare("SELECT verdict,COUNT(*) FROM analysis_gate_decisions WHERE (? IS NULL OR task=?) GROUP BY verdict ORDER BY verdict")?;
    let rows = stmt.query_map(params![task, task], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut counters = json!({"allow":0,"ask":0,"deny":0,"off":0});
    let mut total = 0;
    for row in rows {
        let (k, n) = row?;
        counters[k] = json!(n);
        total += n;
    }
    let mut stmt=conn.prepare("SELECT seq,session_id,index_rev,created_ms,mode,kind,verdict,rule FROM analysis_gate_decisions WHERE (? IS NULL OR task=?) ORDER BY seq DESC LIMIT ?")?;
    let rows=stmt.query_map(params![task,task,limit.min(50) as i64],|r|Ok(json!({"seq":r.get::<_,i64>(0)?,"session_id":r.get::<_,String>(1)?,"index_rev":r.get::<_,Option<i64>>(2)?,"created_ms":r.get::<_,i64>(3)?,"mode":r.get::<_,String>(4)?,"kind":r.get::<_,String>(5)?,"verdict":r.get::<_,String>(6)?,"rule":r.get::<_,String>(7)?})))?;
    let recent = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(
        json!({"decisions":total,"counters":counters,"recent":recent,"recent_truncated":total>limit.min(50) as i64,"effectiveness_measured":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analysis_tasks::TaskSet, models::Document};
    struct Fixture {
        _root: tempfile::TempDir,
        _db: tempfile::TempDir,
        root: PathBuf,
        path: PathBuf,
        store: Store,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let path = root.join("main.rs");
            std::fs::write(&path, "fn main() {}\n").unwrap();
            let db = tempfile::tempdir().unwrap();
            let store = Store::open(&db.path().join("gate.duckdb")).unwrap();
            store
                .upsert_document(&Document {
                    id: "file".into(),
                    uri: format!("file://{}", path.display()),
                    source_file: Some(path.display().to_string()),
                    content: "fn main() {}\n".into(),
                    ..Default::default()
                })
                .unwrap();
            store.lock().unwrap().execute("INSERT INTO source_manifest VALUES (?,?,13,0,?,'file',current_timestamp,current_timestamp)",params![path.display().to_string(),root.display().to_string(),content_hash("fn main() {}\n")]).unwrap();
            store.index_code().unwrap();
            store
                .analysis_task_set(
                    "s",
                    &TaskSet {
                        name: Some("audit".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            Self {
                _root: dir,
                _db: db,
                root,
                path,
                store,
            }
        }
        fn call(&self, mode: Mode) -> Value {
            self.store
                .analysis_gate(
                    "s",
                    &Payload {
                        tool_name: "Bash".into(),
                        tool_input: json!({"command":"rg SECRET_COMMAND_INPUT main.rs"}),
                        cwd: self.root.display().to_string(),
                        ..Default::default()
                    },
                    mode,
                    &[self.root.clone()],
                )
                .unwrap()
        }
        fn use_index(&self, ok: bool, records: Option<i64>) {
            let id = self.store.analysis_call_start("s", "symbol").unwrap();
            self.store.analysis_call_finish(id, ok, records).unwrap();
        }
    }
    #[test]
    fn shell_ast_ignores_strings_comments_and_recognizes_real_commands() {
        for (cmd, kind) in [
            ("echo 'rg foo'", "other"),
            ("printf grep", "other"),
            ("git status", "other"),
            ("git grep foo", "file_search"),
            ("git -C /repo grep foo", "file_search"),
            ("Get-ChildItem -Recurse -Filter *.rs", "file_search"),
            ("# rg foo\ncargo test", "other"),
            ("ls README.md", "other"),
            ("rg foo | sort", "file_search"),
            ("grep x file", "file_search"),
            ("env FOO=bar rg x", "file_search"),
            ("bash -c 'rg foo'", "file_search"),
            ("ls -laR", "file_search"),
            ("echo $(rg foo)", "file_search"),
            ("$PROGRAM foo", "unknown_shell"),
            ("sudo -u someone rg x", "unknown_shell"),
        ] {
            let c = classify(&Payload {
                tool_name: "Bash".into(),
                tool_input: json!({"command":cmd}),
                ..Default::default()
            })
            .unwrap();
            assert_eq!(c.kind, kind, "{cmd}");
        }
        let c = classify(&Payload {
            tool_name: "Read".into(),
            tool_input: json!({"file_path":"main.rs"}),
            ..Default::default()
        })
        .unwrap();
        assert!(c.is_code);
        assert_eq!(
            classify(&Payload {
                tool_name: "ragraf_fake".into(),
                ..Default::default()
            })
            .unwrap()
            .kind,
            "other"
        );
    }
    #[test]
    fn real_completed_nonempty_calls_required_and_ask_escalates_per_task() {
        let f = Fixture::new();
        let r = f.call(Mode::Warn);
        assert_eq!(r["verdict"], "allow");
        assert_eq!(r["facts"]["fresh"], true);
        assert_eq!(r["rule"], "index_unused");
        let fake = Payload {
            tool_name: "ragraf_symbol".into(),
            ..Default::default()
        };
        f.store
            .analysis_gate("s", &fake, Mode::Warn, &[f.root.clone()])
            .unwrap();
        f.use_index(false, Some(10));
        f.use_index(true, Some(0));
        f.store.analysis_call_start("s", "symbol").unwrap();
        assert_eq!(f.call(Mode::Ask)["verdict"], "ask");
        assert_eq!(f.call(Mode::Ask)["verdict"], "deny");
        f.store
            .analysis_task_set(
                "s",
                &TaskSet {
                    name: Some("new".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(f.call(Mode::Ask)["verdict"], "ask");
        f.use_index(true, Some(1));
        let r = f.call(Mode::Ask);
        assert_eq!(r["verdict"], "allow");
        assert_eq!(r["rule"], "index_used");
        let stats = f
            .store
            .analysis_task_stats("s", &Default::default())
            .unwrap();
        assert_eq!(stats["gate"]["decisions"], 2);
        assert_eq!(stats["gate"]["counters"]["ask"], 1);
        let text: String = f
            .store
            .lock()
            .unwrap()
            .query_row(
                "SELECT facts_json FROM analysis_gate_decisions ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!text.contains("SECRET_COMMAND_INPUT"));
        f.store
            .analysis_gate(
                "s",
                &Payload {
                    hook_event_name: "SessionStart".into(),
                    ..Default::default()
                },
                Mode::Warn,
                &[f.root.clone()],
            )
            .unwrap();
        let reset = f.call(Mode::Ask);
        assert_eq!(reset["verdict"], "ask");
        assert_eq!(reset["facts"]["index_used"], false);
    }
    #[test]
    fn changed_bytes_even_same_length_and_old_revision_never_qualify() {
        let f = Fixture::new();
        f.use_index(true, Some(1));
        assert_eq!(f.call(Mode::Deny)["verdict"], "allow");
        std::fs::write(&f.path, "fn next() {}\n").unwrap();
        let r = f.call(Mode::Deny);
        assert_eq!(r["verdict"], "deny");
        assert_eq!(r["rule"], "stale_index");
        let mut doc = f.store.get_document("file").unwrap().unwrap();
        doc.content = "fn next() {}\n".into();
        f.store.upsert_document(&doc).unwrap();
        f.store.index_code().unwrap();
        let r = f.call(Mode::Deny);
        assert_eq!(r["facts"]["fresh"], true);
        assert_eq!(r["rule"], "index_unused");
        assert_eq!(r["verdict"], "deny");
    }
    #[test]
    fn bom_sources_use_decoded_checksums_but_charge_original_byte_budget() {
        let f = Fixture::new();
        let bytes = [
            vec![0xff, 0xfe],
            "fn main() {}\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ]
        .concat();
        std::fs::write(&f.path, &bytes).unwrap();
        f.use_index(true, Some(1));
        let result = f.call(Mode::Deny);
        assert_eq!(result["facts"]["fresh"], true, "{result}");
        assert_eq!(result["verdict"], "allow");
        assert_eq!(result["facts"]["checked_bytes"], bytes.len());
        assert_eq!(
            result["facts"]["checked_bytes_basis"],
            "original_file_bytes"
        );
        std::fs::write(&f.path, [0xff, 0xfe, 0]).unwrap();
        let result = f.call(Mode::Deny);
        assert_eq!(result["rule"], "freshness_unknown");
        assert!(result["facts"]["fresh"].is_null());
    }

    #[test]
    fn unavailable_sources_do_not_claim_fresh_and_reads_remain_allowed() {
        let f = Fixture::new();
        std::fs::write(&f.path, vec![b'a'; 4 * 1024 * 1024 + 1]).unwrap();
        let r = f.call(Mode::Deny);
        assert!(r["facts"]["fresh"].is_null());
        assert_eq!(r["rule"], "freshness_unknown");
        assert_eq!(r["verdict"], "deny");
        let p = Payload {
            tool_name: "Read".into(),
            cwd: f.root.display().to_string(),
            tool_input: json!({"path":"main.rs"}),
            ..Default::default()
        };
        let r = f
            .store
            .analysis_gate("s", &p, Mode::Deny, &[f.root.clone()])
            .unwrap();
        assert_eq!(r["verdict"], "allow");
        let r = f.store.analysis_gate("s", &p, Mode::Off, &[]).unwrap();
        assert_eq!(r["verdict"], "off");
        assert_eq!(r["facts"]["checked_bytes"], 0);
    }
    #[test]
    fn sessions_time_windows_and_missing_files_are_explicit() {
        let f = Fixture::new();
        f.use_index(true, Some(1));
        f.store
            .lock()
            .unwrap()
            .execute("UPDATE analysis_calls SET finished_ms=0", [])
            .unwrap();
        assert_eq!(f.call(Mode::Deny)["rule"], "index_unused");
        std::fs::remove_file(&f.path).unwrap();
        assert_eq!(f.call(Mode::Deny)["facts"]["fresh"], false);
        let r = f
            .store
            .analysis_gate(
                "different",
                &Payload {
                    tool_name: "Bash".into(),
                    cwd: f.root.display().to_string(),
                    tool_input: json!({"command":"rg x"}),
                    ..Default::default()
                },
                Mode::Ask,
                &[f.root.clone()],
            )
            .unwrap();
        assert_eq!(r["verdict"], "ask");
    }
    #[test]
    fn oversized_manifest_is_unknown_after_bounded_live_reads() {
        let f = Fixture::new();
        for i in 0..MAX_FILES {
            let path = f.root.join(format!("{i:03}.txt"));
            std::fs::write(&path, "small").unwrap();
            f.store
                .upsert_document(&Document {
                    id: format!("extra-{i}"),
                    uri: format!("file://{}", path.display()),
                    source_file: Some(path.display().to_string()),
                    content: "small".into(),
                    ..Default::default()
                })
                .unwrap();
        }
        f.store.index_code().unwrap();
        let r = f.call(Mode::Deny);
        assert!(r["facts"]["fresh"].is_null(), "{r}");
        assert_eq!(r["facts"]["checked_files"], MAX_FILES);
        assert_eq!(r["verdict"], "deny");
        assert!(r["facts"]["freshness_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "LIVE_FILE_BUDGET_EXCEEDED"));
    }

    #[test]
    fn hook_contract_does_not_execute_or_rewrite_calls_and_input_is_bounded() {
        let d = Decision {
            verdict: "deny".into(),
            rule: "test".into(),
            reason: "denied".into(),
            context: "context".into(),
            facts: json!({}),
        };
        assert_eq!(d.hook()["exit_code"], 2);
        assert_eq!(d.hook()["stderr"], "denied");
        assert_eq!(d.hook()["stdout"], "");
        let ask = Decision {
            verdict: "ask".into(),
            ..d
        };
        let hook = ask.hook();
        let out: Value = serde_json::from_str(hook["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "ask");
        assert!(out["hookSpecificOutput"].get("updatedInput").is_none());
        assert!(Mode::parse("silent_fallback").is_err());
        assert!(parse_payload(&json!({})).is_err());
        assert!(
            parse_payload(&json!({"tool_name":"Bash","tool_input":"x".repeat(65536)})).is_err()
        );
    }
}
