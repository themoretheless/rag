# Information organization methods (rag-mcp)

**Status:** product design (implement after graph-p0 substrate)  
**Problem:** ingest + search alone leave a **pile**. Agents and humans need ways to **order, group, rank, merge, and compile** knowledge so it compounds (Karpathy) and stays navigable (Obsidian / MemPalace).

This doc defines **organization as a first-class layer** above raw storage and retrieval. Product principle: [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §1.14 · vision loops: [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) §4.3 · map: [`SYSTEM_MAP.md`](SYSTEM_MAP.md).

---

## 1. What “упорядочивание” means here

Five complementary operations (not one feature):

| Family | Question it answers | Examples |
|--------|---------------------|---------|
| **A. Place** | Where does this belong? | wing / room / folder / collection / tags |
| **B. Rank** | What is more important / newer / relevant? | pin, boost, sort, recency, manual rank |
| **C. Structure** | What is the outline / hierarchy / order? | TOC, outline pages, parent-child, topo sort by deps |
| **D. Compile** | How do fragments become durable knowledge? | entity pages, consolidate, file_answer, source_summary |
| **E. Hygiene** | What is redundant / broken / orphaned? | dedupe, merge, lint, refile, archive |

Search finds needles. **Organization keeps the haystack shaped.**

---

## 2. Principles

1. **Raw stays immutable.** Organization mutates **placement metadata**, **wiki pages**, **collections**, and **graph edges** — not silent rewrites of raw source bodies (except explicit re-ingest).
2. **Agent + human.** Tools must work for LLM agents (MCP) and remain inspectable (SQL columns / markdown frontmatter / index.md).
3. **Deterministic first, LLM optional.** Refile by rules and graph is server-side; deep synthesis (`consolidate_*`) is agent-driven with server hooks.
4. **Every organize op is logged** (`ops_log`) with before/after when useful.
5. **Idempotent where possible** (refile same target twice = no-op).
6. **Backend-agnostic** via future `Storage` trait (DuckDB / markdown vault / Postgres).

---

## 3. Data model additions

### 3.1 Placement (already partial in schema)

On `documents` (and vault frontmatter):

| Field | Role |
|-------|------|
| `wing` | top shelf (project, area, person) |
| `room` | sub-area |
| `layer` | `raw` \| `wiki` \| `diary` |
| `kind` | `document` \| `entity` \| `concept` \| `source_summary` \| `outline` \| `collection` \| … |
| `tags` | via graph `tagged` edges (not only free text) |
| `source_file` | provenance path |
| `pinned` | bool — always surfaces in lists / wake_up |
| `boost` | f64 default 1.0 — multiplies rank in search/list |
| `sort_key` | optional string/int for manual order within a room |
| `status` | `active` \| `draft` \| `archived` \| `tombstone` |
| `parent_id` | optional hierarchical parent document (outline tree) |

### 3.2 Collections (ordered lists)

```sql
CREATE TABLE IF NOT EXISTS collections (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  description VARCHAR,
  wing VARCHAR,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS collection_items (
  collection_id VARCHAR NOT NULL,
  document_id VARCHAR NOT NULL,
  position INTEGER NOT NULL,  -- dense order 0..n
  note VARCHAR,
  PRIMARY KEY (collection_id, document_id)
);
```

Use cases: reading list, sprint notes, “sources for thesis X”, curated RAG pack.

### 3.3 Outline pages

A document with `kind=outline` whose body is a structured list of links:

```markdown
# Architecture outline
1. [[Storage adapters]]
2. [[Object graph]]
3. [[Hybrid search]]
```

Server can parse positions → edges `rel_type=outline_child` with `weight=position` or use `parent_id` + `sort_key`.

### 3.4 Merge / alias registry

```sql
CREATE TABLE IF NOT EXISTS aliases (
  alias_label VARCHAR PRIMARY KEY,  -- normalized
  target_node_id VARCHAR NOT NULL
);
```

Supports “LLM Wiki” vs “llm-wiki” as same entity after `merge_nodes`.

---

## 4. Method catalog (APIs / MCP tools)

### A. Place

| Tool | Behavior |
|------|----------|
| `refile` | set `wing`, `room`, `layer` (if allowed), `kind`, `sort_key` on document(s) |
| `set_tags` | replace or add/remove tag edges |
| `list_taxonomy` | wings, rooms, counts (tree) |
| `list_by_placement` | filter wing/room/layer/kind/status + sort |
| `move_to_parent` | set `parent_id` (tree); cycle check |
| `archive` / `unarchive` | status flip; excluded from default search unless `include_archived` |

### B. Rank

| Tool | Behavior |
|------|----------|
| `pin` / `unpin` | `pinned=true` |
| `set_boost` | manual relevance multiplier |
| `reorder` | set `sort_key` or collection `position` for an ordered set |
| `list_documents` sort | `sort=updated\|created\|title\|boost\|manual\|degree` |
| `search` rank fusion | already RRF; add `boost` and `pinned` priority, optional `recency_half_life_days` |

**List order defaults:** pinned first → boost desc → `sort_key` asc → `updated_at` desc.

### C. Structure

| Tool | Behavior |
|------|----------|
| `build_outline` | create/update `kind=outline` page from ids or query seeds |
| `get_outline` | return ordered tree for parent or outline doc |
| `topo_order` | order nodes by dependency edges (`depends_on` / custom rel); report cycles |
| `cluster_suggest` | **server-side light:** group by shared tags / wing or embedding k-means-ish (optional); returns **suggestions**, does not auto-write |
| `suggest_refile` | propose wing/room from similar docs (embedding kNN labels) |

### D. Compile (ordering as synthesis)

| Tool | Behavior |
|------|----------|
| `file_answer` | write Q&A into wiki (Karpathy compounding) |
| `consolidate` | inputs: doc ids or query; **returns** a proposed wiki page body + suggested links (agent may edit); optional `apply=true` writes |
| `compile_source` | one raw source → source_summary wiki + entity touch list (hooks; LLM outside or via client) |
| `rebuild_index` | regenerate catalog ordered by category/wing |
| `read_index` | ordered catalog for navigation-first query |

### E. Hygiene

| Tool | Behavior |
|------|----------|
| `check_duplicate` / `find_near_duplicates` | exact hash + optional embedding similarity threshold |
| `merge_documents` | keep target, redirect uri/aliases, move edges, delete/archive source |
| `merge_nodes` | graph-level alias + edge rewiring |
| `dedupe_edges` | unique (source, target, rel_type) |
| `lint_organization` | empty wings, orphan docs, unsorted collections, stubs aging, archives without reason |
| `normalize_labels` | casefold titles, trim, NFC unicode |

---

## 5. Workflows (how an agent should organize)

### 5.1 After ingest (automatic, cheap)

1. content_hash dedupe warning  
2. extract graph (wikilinks/tags)  
3. if `wing`/`room` provided in metadata → place  
4. append `ops_log` ingest entry  
5. optional: `suggest_refile` only if placement empty (suggestion in result, not auto)

### 5.2 Weekly “lint + order” (agent ritual)

1. `lint_organization` + `lint_wiki`  
2. `find_near_duplicates` → human/agent `merge_*`  
3. `rebuild_index`  
4. for hot topics: `consolidate` → `file_answer` / wiki pages  
5. pin evergreen pages; archive noise  

### 5.3 Project pack (curated context)

1. `create_collection("proj-x")`  
2. `search` / `graph_expand_search` → add items with `collection_add` + positions  
3. `pack_context` from collection order (stable pack for agents)  
4. optional outline page linking the same set  

### 5.4 Dependency ordering

1. authors mark `depends_on` via `link_nodes` or wiki `[[A]] depends on [[B]]` convention (extract optional)  
2. `topo_order(seed)` for reading order / build order  
3. egui **dependency mode** uses same edge types (see `EGUI_GRAPH_VIEW.md` when ready)

---

## 6. Sorting & ranking rules (normative)

### Search final score (conceptual)

```
score = rrf(vec, lex)
        * document.boost
        * (pinned ? pin_bonus : 1)
        * recency_factor(updated_at, half_life)
        * layer_weight(wiki > raw optional)
```

Defaults: `pin_bonus=1.15`, recency off unless `recency_half_life_days` set, layer_weight equal in v1.

### List / index order

1. `status=active` before archived (unless requested)  
2. `pinned`  
3. `boost` desc  
4. `sort_key` asc (manual)  
5. `updated_at` desc  
6. `title` asc tie-break  

### Collection order

Strict `position` integers; tools `collection_add`, `collection_remove`, `collection_reorder(positions[])`.

---

## 7. MCP surface (phased)

### P0.5 (as soon as hybrid/wiki land) — minimum organize kit

- `refile`, `list_by_placement`, `list_taxonomy`  
- `pin` / `set_boost`  
- `archive`  
- `check_duplicate`  
- sort params on `list_documents` / `search`  
- ops_log on these mutations  

### P1 — structure & compile

- collections CRUD + reorder  
- `build_outline` / `get_outline` / `move_to_parent`  
- `consolidate` (propose + apply)  
- `merge_documents` / `merge_nodes`  
- `find_near_duplicates`  
- `topo_order`  
- `lint_organization`  
- `pack_context` from collection  

### P2 — smart assist

- `cluster_suggest`, `suggest_refile` (embedding)  
- auto-outline from community detection  
- multi-agent “librarian” prompt resource  
- vault folder moves mirrored to wing/room  

---

## 8. Interaction with other layers

| Layer | Organize uses it |
|-------|------------------|
| Graph | tags, stubs, outline edges, depends_on, merge rewires |
| Wiki (Karpathy) | consolidate / file_answer / index are organization |
| Hybrid search | boost/pin/recency; collections as filter `in_collection=` |
| Markdown vault | refile ↔ path under `wing/room/`; collections as `wiki/collections/*.md` Dataview-friendly |
| Egui | color by wing; size by boost; filter archived; dependency topo layout |

---

## 9. Non-goals

- Silent LLM rewrite of all notes on every ingest  
- Automatic destructive merges without explicit tool call  
- Full desktop “Notion databases” UI in MCP  
- Guaranteed global ontology enforced by server (schema.md is advisory + optional validators)

---

## 10. Acceptance tests (design)

1. Refile two docs into wing `research/room=rag` → `list_by_placement` returns both ordered by pin/boost.  
2. Collection of 3 docs with positions 0,1,2 → reorder → `pack_context` follows new order.  
3. Merge A into B → search/uri of A resolves or is gone; edges point to B; alias works.  
4. Outline page parse → `get_outline` order matches body list.  
5. `topo_order` on chain A→B→C returns C,B,A or A,B,C per convention (document direction); cycle returns error + cycle path.  
6. Archive doc → default search excludes; `include_archived=true` includes.  

---

## 11. Summary

**Organization is not only folders.** It is:

- **place** (taxonomy),  
- **rank** (pin/boost/sort),  
- **structure** (outline/tree/collections/topo),  
- **compile** (wiki consolidate),  
- **hygiene** (dedupe/merge/lint).  

Implement in that order: placement+rank first (cheap, high leverage), then collections/outlines, then merge/consolidate, then smart suggestions.
