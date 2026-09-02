# Local LLM + storage maintenance (rag-mcp)

**Goal:** run models **on your machine** (Ollama / LM Studio / llama.cpp OpenAI-compatible), then use them to **analyze, organize, improve, compress, and refresh** both the wiki and the DuckDB (or vault) store.

**Status:** historical design and operating notes. Current shipped behavior is
owned by [`README.md`](../README.md), [`SYSTEM_MAP.md`](SYSTEM_MAP.md), and the
code. The phase labels below record the original delivery sequence; they are not
an active backlog or a source of roadmap commitments.

**Product stance:** optional server-side chat for maintain/compile helpers, **not** mandatory graph NER. Default knowledge author remains the MCP client LLM via wiki tools. See [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §1.9 · [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) §8.

---

## 1. User story

> «Локально поднять LLM, пройтись по базе/вики, навести порядок, сжать, актуализировать — без облака.»

Pipeline:

```
local embedder + local chat LLM
        │
        ▼
┌───────────────────┐
│  analyze_corpus   │  health, dups, orphans, stale wiki, size
└─────────┬─────────┘
          ▼
┌───────────────────┐
│  plan_maintenance │  LLM proposes ordered actions (JSON)
└─────────┬─────────┘
          ▼
┌───────────────────┐
│ apply (dry_run?)  │  refile, merge, consolidate, archive, reembed
└─────────┬─────────┘
          ▼
┌───────────────────┐
│  compress_store   │  dedupe, vacuum, drop tombstones, rebuild FTS
└─────────┬─────────┘
          ▼
┌───────────────────┐
│  refresh_wiki     │  outdated source_summary, index.md, log
└───────────────────┘
```

Every step logs to `ops_log`. Destructive steps require `confirm=true` or `dry_run` first.

---

## 2. Local model providers

### 2.1 Chat (reasoning / organize / compress text)

Already sketched in config:

| Env | Default | Meaning |
|-----|---------|---------|
| `RAG_LLM_ENABLED` | `true` | allow chat tools |
| `RAG_LLM_BASE_URL` | `http://127.0.0.1:11434/v1` | Ollama OpenAI-compatible |
| `RAG_LLM_MODEL` | `llama3.2` | model tag |
| `RAG_LLM_API_KEY` | `ollama` | optional Bearer |

Also works with:

- LM Studio: `http://127.0.0.1:1234/v1`
- llama.cpp server: `http://127.0.0.1:8080/v1`
- Any OpenAI-compatible local proxy

API: `POST {base}/chat/completions` (existing `src/llm/chat.rs`).

### 2.2 Embeddings (local)

Extend `RAG_EMBEDDING_PROVIDER`:

| Value | Endpoint |
|-------|----------|
| `mock` | deterministic (tests) |
| `openai` | cloud or compatible |
| **`ollama`** | `POST {base}/api/embeddings` or `/v1/embeddings` |
| **`openai_compat`** | same as openai, for local servers |

Recommended local pair:

```bash
ollama pull nomic-embed-text   # or mxbai-embed-large
ollama pull llama3.2           # or qwen2.5, mistral, …

export RAG_EMBEDDING_PROVIDER=ollama   # or openai + BASE_URL to Ollama /v1
export RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434/v1
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768
export RAG_EMBEDDING_API_KEY=ollama

export RAG_LLM_BASE_URL=http://127.0.0.1:11434/v1
export RAG_LLM_MODEL=llama3.2
export RAG_LLM_ENABLED=true
```

`reembed_document` is refresh-only: it may update one document only while the
stored and configured corpus identities (provider, model, dimensions, and base
endpoint) match. If any identity field changes, vector/hybrid search stays
closed until a complete, uncapped `reembed_all` succeeds; only then is the
target manifest published. Set `max_docs` to at least the current corpus
document count and raise the gateway's `RAG_MAINT_MAX_DOCS` to the same value
before restart, because per-call `max_docs` is clamped to that hard cap. The
default maintenance cap refuses a larger migration before changing any vectors.
Once migration starts, a persistent incompatible marker
precedes the first vector write, so partial failure or configuration rollback
cannot reopen vector/hybrid search over mixed identities.

### 2.3 Policy

- **Offline-first:** if LLM unreachable and tool needs it → clear error, not hang forever (timeouts).
- **No cloud by default** when base_url is loopback.
- Token budget env: `RAG_LLM_MAX_TOKENS`, `RAG_LLM_TIMEOUT_SECS`.

---

## 3. Maintenance capabilities

### 3.1 Analyze (`analyze_corpus`)

Deterministic SQL/graph (no LLM required):

| Signal | Source |
|--------|--------|
| counts | docs/chunks/nodes/edges by layer/wing/kind/status |
| orphans | graph nodes degree 0; wiki pages never linked |
| stubs aging | unresolved stubs older than N days |
| near-dups | same content_hash; optional embedding cosine > θ |
| stale wiki | wiki page `updated_at` < linked raw `updated_at` |
| embed mismatch | manifest vs config |
| FTS health | doctor |
| size | db file bytes, avg chunk len |
| archive candidates | low boost, old updated_at, never retrieved (if metrics) |

Output: structured `AnalysisReport` JSON.

Optional LLM layer: `analyze_corpus_llm` summarizes report + prioritizes fixes in natural language.

### 3.2 Organize wiki + DB (`maintain_organize`)

Uses ORGANIZE.md + local LLM:

| Action | dry_run | apply |
|--------|---------|-------|
| suggest refile (wing/room) | yes | `refile` batch |
| rebuild index catalog | yes | `rebuild_index` |
| build outline for a wing | yes | write outline doc |
| pin evergreen / archive noise | suggestions | pin/archive |
| tag normalization | yes | set_tags |

LLM prompt inputs: taxonomy snapshot + sample titles + analysis issues.  
Output must be **JSON action list** validated by server (whitelist ops only).

### 3.3 Improve knowledge (`maintain_improve`)

| Action | Description |
|--------|-------------|
| `compile_source` | raw → source_summary wiki (existing path) |
| `consolidate` | cluster of docs → one concept/entity page |
| `file_answer` | store Q&A into wiki |
| `refresh_stale_wiki` | recompile wiki pages whose raw parents changed |
| `lint_and_fix_links` | propose missing `[[links]]` / resolve stubs |
| `kg_extract` optional | LLM triples → kg_add (off by default; opt-in) |

Always: raw immutable; wiki is the mutable compiled layer (Karpathy).

### 3.4 Compress storage (`maintain_compress`)

| Level | Action | Risk |
|-------|--------|------|
| L0 safe | FTS reindex; DuckDB `CHECKPOINT`; drop empty ops older than N (optional) | low |
| L1 | exact hash dedupe merge; delete duplicate chunks after merge | medium |
| L2 | near-duplicate merge (embedding θ) with LLM pick canonical | medium |
| L3 | archive low-value docs; soft-delete tombstones purge after grace | medium |
| L4 | compress diary episodes → one weekly wiki page; delete raw diaries (opt-in) | high |
| L5 | re-chunk with larger size; reembed (size ↓ metadata) | high |

`compress` never deletes raw `layer=raw` without `allow_raw_delete=true`.

Report: bytes before/after, docs removed, chunks removed, actions applied.

### 3.5 Actualize / refresh (`maintain_refresh`)

| Action | When |
|--------|------|
| `reembed` / `reembed_document` | Refresh one document while the stored and configured corpus identities match |
| `reembed_all` | Provider, model, dimensions, or base endpoint changed; raise `RAG_MAINT_MAX_DOCS` and set the clamped per-call `max_docs` to at least the current corpus document count; publish the target manifest only after full success |
| `reindex_fts` | after bulk deletes |
| `rebuild_graph` | after bulk meta edits |
| `rebuild_index` | wiki catalog stale |
| `sync_vault` | markdown backend mtime drift (P1) |
| `wake_up` snapshot | agent session start |

---

## 4. MCP tools (maintenance surface)

| Tool | Role |
|------|------|
| `llm_status` | ping local LLM + model name; embedding provider status |
| `analyze_corpus` | deterministic report |
| `analyze_corpus_nl` | LLM narrative + prioritized plan |
| `plan_maintenance` | LLM → validated JSON action plan from report |
| `apply_maintenance_plan` | execute plan with `dry_run` flag |
| `maintain_organize` | one-shot organize pass |
| `maintain_compress` | compression levels L0–L3 default |
| `maintain_refresh` | reembed/fts/graph/index flags |
| `consolidate` | LLM consolidate docs → wiki page |
| `refresh_stale_wiki` | batch recompile |
| `vacuum_store` | CHECKPOINT + stats |
| `maintenance_log` | read recent maintain ops |

CLI (optional binary feature):

```bash
rag-mcp maintain --dry-run
rag-mcp maintain --apply --level 2
rag-mcp analyze
```

---

## 5. Safety

1. **Whitelist actions** — LLM cannot invent SQL or shell.  
2. **dry_run default** for multi-action plans.  
3. **confirm** for L2+ compress.  
4. **Budget:** max docs touched per run (`RAG_MAINT_MAX_DOCS=50`); per-call
   `max_docs` and duplicate-cleanup `max_candidates` can select a smaller batch
   but cannot exceed that configured hard cap. An identity migration must raise
   the gateway env cap and explicitly set `max_docs` to at least the current
   corpus document count, or it is refused before mutation.
5. **ops_log** every apply.  
6. **Backup recommendation** in tool description; optional `backup_db` before apply.  
7. **No silent raw mutation.**

---

## 6. Module layout

```
src/llm/           # chat + (future) ollama embed helper
src/embeddings/ollama.rs
src/maintain/
  mod.rs
  analyze.rs       # deterministic signals
  plan.rs          # JSON schema for actions
  apply.rs         # execute whitelist
  compress.rs
  refresh.rs
  prompts.rs       # system prompts for organize/compress/improve
```

---

## 7. Historical delivery sequence

These labels explain how the maintenance surface arrived. Current scheduling is
only in [`ROADMAP.md`](ROADMAP.md).

| Phase | Deliverable |
|-------|-------------|
| **M0** (quick) | Document env; `llm_status`; ensure Ollama chat works with existing compile; ollama embed provider |
| **M1** | `analyze_corpus` + `vacuum_store` + `maintain_refresh` (no LLM) |
| **M2** | `plan_maintenance` + `apply_maintenance_plan` dry_run/apply; organize actions |
| **M3** | `maintain_compress` L0–L2; near-dup merge |
| **M4** | `refresh_stale_wiki` + consolidate batch; CLI `maintain` |
| **M5** | metrics of retrieval, auto schedule (external cron calling MCP/CLI) |

---

## 8. Example session (shared local gateway)

```bash
# Terminal 1
ollama serve
ollama pull llama3.2
ollama pull nomic-embed-text

# Configure these values on the one local.rag-mcp gateway, then restart it.
# Do not launch a second rag-mcp process.
export RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb
export RAG_EMBEDDING_PROVIDER=ollama
export RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768
export RAG_EMBEDDING_API_KEY=ollama
# Alternative: openai + local BASE_URL also works (OpenAI-compatible /v1/embeddings):
#   RAG_EMBEDDING_PROVIDER=openai  (or openai_compat)
#   RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434/v1
# Or: RAG_EMBEDDING_PROVIDER=ollama with BASE_URL ending in /v1 (uses same OpenAI path)
export RAG_LLM_BASE_URL=http://127.0.0.1:11434/v1
export RAG_LLM_MODEL=llama3.2

# After the configured gateway is restarted:
curl -sS http://127.0.0.1:7432/ready
```

Agent tools:

1. `llm_status` → ok  
2. `analyze_corpus` → 12 stubs, 3 near-dups, 2 stale wiki  
3. `plan_maintenance` → JSON  
4. `apply_maintenance_plan` dry_run → preview  
5. `apply_maintenance_plan` apply=true  
6. `maintain_compress` level=1  
7. `vacuum_store`  

---

## 9. Relation to other docs

| Doc | Overlap |
|------|---------|
| ORGANIZE.md | place/rank/structure ops used inside maintain |
| MEMPALACE_PARITY.md | wake_up/diary; maintain can write diary of runs |
| Karpathy wiki | compile/refresh_stale_wiki is the maintenance of compiled layer |
| STORAGE_ADAPTERS | vacuum/export differ per backend |

---

## 10. Success criteria

- Full offline loop on Ollama (embed + chat) with mock fallback for CI.  
- One `analyze` → `plan` → `dry_run` → `apply` → `compress` path documented and tested.  
- DB size and orphan/stub counts decrease on a fixture corpus after compress.  
- Wiki index reflects post-maintain state.  
- No raw document deleted without explicit flag.
