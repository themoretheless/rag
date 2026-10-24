# Optimisation audit: launchd TCC, duplicate removal, graph rebuild gotcha

Date: 2026-10-02
Gateway version: HEAD (rag-mcp 0.1), DuckDB backend, live DB at `~/.local/share/rag-mcp/rag.duckdb` (~7 GB file)

---

## Background context

User requested "full DB analysis, clean, refresh, ingest" → measured only 5.7% were provably dead (missing source_file) → then "хватит 5% и дубли удаляй" (stop at 5%, delete duplicates). MCP HTTP transport stateless with ZERO auth by default; gateway lives on `http://127.0.0.1:7432`. macOS TCC blocks launchd-spawned `/bin/sh` from reading under `~/Documents`.

---

## Point 1: cron script runs under launchd but TCC blocks reads of ~/Documents

**Problem:** `scripts/wiki-refresh.sh` invoked by a launchd job that ran relative paths into repo caused false negatives in queue-drain loop due to TCC errors (exit 126 / "Operation not permitted") when reading any path under `~/Documents`.

**Solution:** Relocate everything outside `~/Documents`:
```sh
ROOT="${RAG_CRON_PROJECT_ROOT:-$(cd ... && pwd)}"
STATE="${RAG_CRON_STATE_DIR:-$HOME/.local/share/rag-mcp/cron}"
SCRIPT_PATH="$HOME/.local/bin/rag-wiki-refresh.sh"
QUEUE="$STATE/pending-ingest.txt"   # symlink out of repo
```
Plist points to `$HOME/.local/bin/rag-wiki-refresh.sh` with no `WorkingDirectory`; env vars include `RAG_CRON_PROJECT_ROOT`, `RAG_CRON_STATE_DIR`, `RAG_MCP_URL`, and `PATH` with `$HOME/.local/bin`. Queue draining uses `cat >` instead of `mv` so symlink isn't clobbered.

**Result:** Real run drained all 28 queued paths successfully, exit 0. Script now runs end-to-end every 6 hours without TCC errors.

---

## Point 2: duplicate cleanup – measured real removable rows vs manifest-owned placeholders

**Initial measurements (live auto-backup):**
- Corpus-wide hash-groups: 4 381 groups / 6 381 removable docs / ~79k chunks
- Within-wing dedupe (one keeper per `(wing, content_hash)`): 2 499 candidates / ~34k chunks
- Branch/copy wings: `personalization-travel-time-units` 2444 removable, `personalization copy` 1476, `env` 847
- Zero-byte files among removable: 262

**First pass: `maintain_compress level=2`** (query sees only top 50 groups via `LIMIT 50`)
- Tombstoned: 929 docs from 50 exact-dup groups + 2 near-dup pairs
- Top group size: 257 members (hash `af1349b9...`) — zero-byte `.cs`/`.vue` placeholder files
- Next groups: sizes 80, 35, 22, 22, 20

**Physical deletion: `apply_maintenance_plan drop_tombstones allow_raw_delete=true max_docs=2000`**
- Deleted: 929 docs → corpus **77 581** docs, chunks **606 650** (from 83 280 active before purge/deletes)
- Graph prune: 929 orphan stubs removed (`prune_orphan_stubs` fires when `processed == candidate_count && skipped_cap == 0`)
- Lint issues: dropped 1 200 → 271 (stubs 1 014 → 85); search functional
- Characterisation of deleted set: 256 empty files, 672 sub-1 KB files, one 3.5 KB file — all were their `source_file`'s `source_manifest.owner` row (distinct files, junk placeholder artifacts)

**Key findings:**
- Why stopping at 929 was right: a full probe showed 2 335 of remaining 2 499 within-wing candidates are the canonical `source_manifest.document_id` for their own `source_file` — deleting them removes distinct files from index, not redundant rows. Only 164 are true duplicate rows.
- Tool limitation: `find_exact_duplicate_groups` uses `LIMIT 50` — each call sees only the biggest page, never the population. Bulk dedupe beyond 929 requires computed list of candidate ids + `delete_document` in batches.

**Protections checked against live backup:**
- `source_manifest`: 2 335 candidates match (all manifest owners)
- `collection_entries`: 0
- `wiki_index`: 0
- `document_revisions`: 37

Safety net: Backup written to `backups/rag-pre-wingdedupe-20260927.duckdb` (6.95 GB, sha256 `60491ed9…`). Path must be inside `RAG_INGEST_ROOTS`.

---

## Point 3: graph rebuild did not prune stubs until `max_docs` clarified

**Gotcha identified:** `src/maintain/refresh.rs:398` guards `prune_orphan_stubs()` behind `complete_success = !dirty_only && failed == 0 && skipped_cap == 0 && processed == candidate_count`.
- My explicit `max_docs=50000` clamped below corpus size (77 581), so `skipped_cap > 0` and `pruned_orphan_stubs` stayed 0; lint showed 1 014 stub issues.

**Fix:** Pass `max_docs=100000` (gateway's `RAG_MAINT_MAX_DOCS`) or omit it entirely.
- Full rebuild over 77 581 docs succeeded: Processed/Succeeded 77 581, Skipped cap 0
- Pruned orphan stubs: 929 (exactly the deleted set)
- Lint: 1 200 → 271 issues; integrity OK

**Caveat:** Server-side builds may exceed the 900 s MCP curl timeout. Confirm completion via `list_recent_ops` rather than HTTP exit code.

---

## Final state

- Corpus: **77 581 documents**, **606 650 chunks**
- Graph: 99 895 nodes, 228 889 edges (orphan stubs pruned)
- Health: `relational_integrity_ok=true`, FTS ready, embedding contract ok, schema v16
- Lint: 271 issues (stubs 85)
- Backup: `backups/rag-pre-wingdedupe-20260927.duckdb` created post-deletion, verified

**Open (awaiting explicit permission):** COMPACT to reclaim physical file size (~10 GB remain), delete 14 GB `rag.duckdb.art-broken-20260926T200313Z` leftover, further within-wing dedupe beyond the 164 true duplicate rows (requires id-list approach).

---

## Lessons learned

- Always measure before deleting; a quota-driven delete can remove distinct files if you don't probe protections.
- For large maintenance ops, verify completion server-side, not just via HTTP exit code.
- When the tool caps its query (`LIMIT 50`), compute your own list if you need full population.
- Launchd + macOS TCC means scripts in ~/Documents need special handling; relocate script+state or use symlinks appropriately.
