//! In-memory call log for MCP tools and HTTP retrieval, plus agent presence.
//!
//! `ops_log` (DuckDB) records **mutations** only. Reads (`search`, `get_neighbors`,
//! `pack_context`, …) are the most frequent thing agents do, so this module keeps a
//! bounded ring buffer of every call with latency and outcome. Nothing here touches
//! the database; the buffer is lost on restart by design.
//!
//! Capacity: `RAG_CALL_LOG_CAPACITY` (default [`DEFAULT_CAPACITY`]).
//! Surfaces: `GET /v1/calls`, `GET /v1/agents`.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

pub const DEFAULT_CAPACITY: usize = 2_000;
/// Agents seen within this window count as online.
pub const ONLINE_WINDOW_SECS: i64 = 600;
const ARGS_MAX_CHARS: usize = 240;
const ERROR_MAX_CHARS: usize = 240;

/// One recorded tool / endpoint invocation.
#[derive(Debug, Clone, Serialize)]
pub struct CallRecord {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    /// MCP client name (`clientInfo.name`) or `http` for gateway REST calls.
    pub agent: String,
    /// `stdio` | `http-mcp` | `http`.
    pub transport: String,
    pub tool: String,
    /// Coarse family for share breakdowns (`search`, `graph`, `wiki`, …).
    pub group: &'static str,
    /// Compact JSON arguments, truncated to a short prefix.
    pub args: String,
    pub elapsed_ms: f64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Short outcome such as `8 hits` or `updated r3`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hint: Option<String>,
}

struct Buffer {
    next_seq: u64,
    capacity: usize,
    records: VecDeque<CallRecord>,
}

static BUFFER: LazyLock<RwLock<Buffer>> = LazyLock::new(|| {
    let capacity = std::env::var("RAG_CALL_LOG_CAPACITY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CAPACITY);
    RwLock::new(Buffer { next_seq: 1, capacity, records: VecDeque::with_capacity(capacity) })
});

/// Started call; call [`CallStart::finish`] to record it.
#[must_use = "a started call is only recorded by finish()"]
pub struct CallStart {
    started: Instant,
    agent: String,
    transport: String,
    tool: String,
    args: String,
}

/// Begin timing one call. `args` is any JSON value (typically the tool parameter object).
pub fn begin(agent: &str, transport: &str, tool: &str, args: Option<&serde_json::Value>) -> CallStart {
    let args = args
        .map(|value| truncate(&value.to_string(), ARGS_MAX_CHARS))
        .unwrap_or_default();
    CallStart {
        started: Instant::now(),
        agent: normalize_agent(agent),
        transport: transport.to_string(),
        tool: tool.to_string(),
        args,
    }
}

impl CallStart {
    pub fn finish(self, ok: bool, error: Option<String>, result_hint: Option<String>) {
        let record = CallRecord {
            seq: 0,
            ts: Utc::now(),
            group: tool_group(&self.tool),
            agent: self.agent,
            transport: self.transport,
            tool: self.tool,
            args: self.args,
            elapsed_ms: self.started.elapsed().as_secs_f64() * 1_000.0,
            ok,
            error: error.map(|message| truncate(&message, ERROR_MAX_CHARS)),
            result_hint,
        };
        push(record);
    }
}

fn push(mut record: CallRecord) {
    let mut buffer = BUFFER.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    record.seq = buffer.next_seq;
    buffer.next_seq += 1;
    if buffer.records.len() >= buffer.capacity {
        buffer.records.pop_front();
    }
    buffer.records.push_back(record);
}

fn normalize_agent(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() { "unknown".into() } else { trimmed.to_string() }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Coarse tool family used by the console for share breakdowns.
pub fn tool_group(tool: &str) -> &'static str {
    match tool {
        "search" | "multi_query_search" | "query_with_index" | "search_wiki" | "pack_context"
        | "find_similar" | "expand_chunks" | "multi_get" | "get_document" | "get_source"
        | "list_documents" | "list_sources" | "check_duplicate" => "search",
        "get_graph" | "get_neighbors" | "get_backlinks" | "find_node" | "graph_expand_search"
        | "link_nodes" | "create_tunnel" | "list_tunnels" | "delete_tunnel" | "follow_tunnels"
        | "find_tunnels" | "graph_stats" | "export_graph_snapshot" => "graph",
        "write_wiki_page" | "update_wiki_page" | "get_wiki_page" | "list_wiki_pages"
        | "file_answer" | "read_index" | "update_index_entry" | "rebuild_index" | "get_schema"
        | "update_schema" | "lint_wiki" | "refresh_stale_wiki" | "compile_source"
        | "consolidate" => "wiki",
        "ingest_text" | "ingest_file" | "ingest_raw" | "sync_sources" | "add_drawer"
        | "delete_document" | "delete_by_source" | "update_document_meta" | "reembed_document" => {
            "ingest"
        }
        "kg_add" | "kg_query" | "kg_invalidate" | "kg_supersede" | "kg_timeline" | "kg_stats" => {
            "kg"
        }
        "diary_write" | "diary_read" | "wake_up" | "checkpoint" | "append_log" | "read_log"
        | "list_recent_ops" | "memories_filed_away" | "list_memory_lifecycle_candidates"
        | "consolidate_memory_items" | "archive_memory_items" => "memory",
        "analyze_corpus" | "plan_maintenance" | "apply_maintenance_plan" | "maintain_organize"
        | "maintain_refresh" | "maintain_compress" => "maintain",
        _ => "ops",
    }
}

/// Newest-first records, optionally filtered by agent and/or tool.
pub fn recent(limit: usize, agent: Option<&str>, tool: Option<&str>) -> Vec<CallRecord> {
    let buffer = BUFFER.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    buffer
        .records
        .iter()
        .rev()
        .filter(|record| agent.is_none_or(|name| record.agent.eq_ignore_ascii_case(name)))
        .filter(|record| tool.is_none_or(|name| record.tool == name))
        .take(limit.max(1))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupShare {
    pub group: &'static str,
    pub count: usize,
    /// Fraction of all buffered calls, 0..=1.
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStats {
    pub tool: String,
    pub count: usize,
    pub errors: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallSummary {
    /// Records currently buffered.
    pub buffered: usize,
    pub capacity: usize,
    /// Calls recorded during the last 60 minutes.
    pub calls_last_hour: usize,
    pub errors: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub by_group: Vec<GroupShare>,
    pub by_tool: Vec<ToolStats>,
}

pub fn summary() -> CallSummary {
    let buffer = BUFFER.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    let records: Vec<&CallRecord> = buffer.records.iter().collect();
    let hour_ago = Utc::now() - Duration::minutes(60);
    let latencies: Vec<f64> = records.iter().map(|record| record.elapsed_ms).collect();
    let mut groups: HashMap<&'static str, usize> = HashMap::new();
    let mut tools: HashMap<&str, Vec<&CallRecord>> = HashMap::new();
    for record in &records {
        *groups.entry(record.group).or_default() += 1;
        tools.entry(record.tool.as_str()).or_default().push(record);
    }
    let total = records.len().max(1) as f64;
    let mut by_group: Vec<GroupShare> = groups
        .into_iter()
        .map(|(group, count)| GroupShare { group, count, share: count as f64 / total })
        .collect();
    by_group.sort_by(|a, b| b.count.cmp(&a.count).then(a.group.cmp(b.group)));
    let mut by_tool: Vec<ToolStats> = tools
        .into_iter()
        .map(|(tool, rows)| {
            let lat: Vec<f64> = rows.iter().map(|record| record.elapsed_ms).collect();
            ToolStats {
                tool: tool.to_string(),
                count: rows.len(),
                errors: rows.iter().filter(|record| !record.ok).count(),
                p50_ms: percentile(&lat, 0.50),
                p95_ms: percentile(&lat, 0.95),
            }
        })
        .collect();
    by_tool.sort_by(|a, b| b.count.cmp(&a.count).then(a.tool.cmp(&b.tool)));
    CallSummary {
        buffered: records.len(),
        capacity: buffer.capacity,
        calls_last_hour: records.iter().filter(|record| record.ts >= hour_ago).count(),
        errors: records.iter().filter(|record| !record.ok).count(),
        p50_ms: percentile(&latencies, 0.50),
        p95_ms: percentile(&latencies, 0.95),
        by_group,
        by_tool,
    }
}

/// Presence derived from the call buffer (one row per agent name).
#[derive(Debug, Clone, Serialize)]
pub struct AgentActivity {
    pub agent: String,
    pub transport: String,
    pub calls_total: usize,
    pub calls_today: usize,
    pub errors: usize,
    pub last_call_at: DateTime<Utc>,
    pub last_tool: String,
    pub p95_ms: f64,
    /// Seen within [`ONLINE_WINDOW_SECS`].
    pub online: bool,
}

pub fn agent_activity() -> Vec<AgentActivity> {
    let buffer = BUFFER.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Utc::now();
    let day_start = now.date_naive().and_hms_opt(0, 0, 0).map(|naive| naive.and_utc()).unwrap_or(now);
    let mut per_agent: HashMap<&str, Vec<&CallRecord>> = HashMap::new();
    for record in &buffer.records {
        per_agent.entry(record.agent.as_str()).or_default().push(record);
    }
    let mut out: Vec<AgentActivity> = per_agent
        .into_iter()
        .map(|(agent, rows)| {
            let last = rows.last().expect("non-empty group");
            let lat: Vec<f64> = rows.iter().map(|record| record.elapsed_ms).collect();
            AgentActivity {
                agent: agent.to_string(),
                transport: last.transport.clone(),
                calls_total: rows.len(),
                calls_today: rows.iter().filter(|record| record.ts >= day_start).count(),
                errors: rows.iter().filter(|record| !record.ok).count(),
                last_call_at: last.ts,
                last_tool: last.tool.clone(),
                p95_ms: percentile(&lat, 0.95),
                online: (now - last.ts).num_seconds() <= ONLINE_WINDOW_SECS,
            }
        })
        .collect();
    out.sort_by_key(|agent| std::cmp::Reverse(agent.last_call_at));
    out
}

fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Drop all buffered records (tests only; the buffer is process-global).
pub fn clear() {
    let mut buffer = BUFFER.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    buffer.records.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_group_and_summarize() {
        clear();
        begin("claude-code", "stdio", "search", Some(&serde_json::json!({"query": "x"})))
            .finish(true, None, Some("3 hits".into()));
        begin("zed", "http-mcp", "get_neighbors", None).finish(false, Some("boom".into()), None);
        let rows = recent(10, None, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tool, "get_neighbors");
        assert_eq!(rows[0].group, "graph");
        assert_eq!(rows[1].args, r#"{"query":"x"}"#);
        let summary = summary();
        assert_eq!(summary.buffered, 2);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.by_group[0].count, 1);
        let agents = agent_activity();
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().all(|agent| agent.online && agent.calls_today == 1));
        assert_eq!(recent(10, Some("ZED"), None).len(), 1);
        assert_eq!(tool_group("kg_add"), "kg");
        assert_eq!(tool_group("status"), "ops");
    }

    #[test]
    fn percentile_and_truncate_are_bounded() {
        assert_eq!(percentile(&[], 0.95), 0.0);
        assert_eq!(percentile(&[5.0, 1.0, 3.0], 0.5), 3.0);
        let long = "a".repeat(500);
        assert_eq!(truncate(&long, 10).chars().count(), 10);
    }
}
