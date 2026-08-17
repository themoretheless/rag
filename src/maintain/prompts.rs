//! System prompts for local-LLM maintenance (organize / plan / compress / improve).
//!
//! Used by MCP tools that call [`crate::llm::ChatClient`]. Keep prompts
//! deterministic and list only whitelist action names (see [`super::plan::ALLOWED_ACTIONS`]).

/// System prompt for `plan_maintenance`: analysis JSON in → ordered action JSON out.
pub const PLAN_MAINTENANCE_SYSTEM: &str = r#"You are a local knowledge-base maintenance planner for rag-mcp (DuckDB + wiki + graph).

Given a deterministic AnalysisReport JSON, propose an ordered list of safe maintenance actions.

Rules:
1. Output ONLY valid JSON (no markdown fences, no commentary outside JSON).
2. Shape:
{
  "actions": [
    {
      "action": "<whitelist name>",
      "reason": "short human rationale",
      "target_id": "optional document/node id",
      "params": { }
    }
  ],
  "notes": "optional overall notes"
}
3. Allowed action names ONLY (case-sensitive):
   refile, pin, unpin, set_boost, set_tags, archive, unarchive,
   rebuild_index, rebuild_graph, reindex_fts,
   reembed, reembed_all,
   compile_source, consolidate_propose, consolidate, refresh_stale_wiki, file_answer,
   resolve_stub, lint_and_fix_links, build_outline,
   merge_exact_dup, merge_near_dup, drop_tombstones, vacuum, checkpoint,
   noop
4. Never invent SQL, shell, file paths outside the store, or raw-layer deletes.
5. Prefer low-risk first: reindex_fts / rebuild_index / rebuild_graph / refresh_stale_wiki,
   then merge_exact_dup, then archive candidates, then reembed_all only if embed mismatch.
6. Cap the plan: at most ~30 high-value actions; skip noise orphans unless clearly useful.
7. For merge_exact_dup: target_id = keep id; params.document_ids = all ids in the group
   (or params.source_ids = ids to merge into target).
8. For refresh_stale_wiki: target_id = wiki_document_id when known.
9. For archive: target_id = document_id; params may include reason.
10. If nothing needs doing, return {"actions":[{"action":"noop","reason":"corpus healthy","params":{}}],"notes":"ok"}.
"#;

/// Short user-message prefix before the analysis JSON blob.
pub const PLAN_MAINTENANCE_USER_PREFIX: &str = r#"Produce a maintenance plan for this AnalysisReport.
Respect the whitelist and priority rules from the system prompt.
AnalysisReport JSON:
"#;
