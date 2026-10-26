# Bounded impact relation traversal

`impact_relations` selects the incoming symbol-dependency graph:

- `calls` (default): existing candidate call links.
- `references`: calls plus bound value/type occurrences with an owning symbol.
- `all`: those links plus class inheritance and inherited method candidates.

No candidate relation becomes a call. Each returned symbol has `depth`,
`via_kind`, `via_target` and its original `via` evidence. Override witnesses still
state semantic_override_proven=false. Ambiguous bound targets remain candidates.
Only known source/target symbols with equal project identity enter the traversal.
Exact seed paths constrain selection, not paths of dependent symbols. An exact
symbol ID can disambiguate identical names/paths across projects.

Breadth-first traversal retains one shortest discovered witness per symbol,
excludes selected seeds from the affected list and handles cycles. Incoming edges
and frontiers use deterministic ordering. The item limit is 1..500; depth is
1..10. `truncated` reports refused result items; `depth_limited` reports remaining
unvisited dependencies at the returned frontier. `traversal_complete` refers only
to exhausting this selected fresh bound-symbol graph within both limits. It does
not prove exhaustive program impact. Invalid relation modes return an explicit
error object, matching the existing reference-mode validation convention.

Freshness/unindexed/parse diagnostics remain visible. `runtime_impact_proven` and
`absence_proven` are false. Module-level occurrences without an owning symbol,
unresolved names, dynamic dispatch, SQL/table/file targets, indirect ORM/dataflow,
reference confidence calibration and stated-versus-derived summary modes remain
outside this traversal. No source body is added to the response. The declared
read contract changes no stored extraction facts: index format remains 29 and
database schema remains 25.

## Affected-file summary

`affected_files` groups returned affected symbols by exact document identity,
with path/project, captured source revision/hash, symbol_count, min_depth and
relation_counts for the retained first witnesses. Rows sort by descending symbol
count, then path and document ID. Identical paths in distinct documents/projects
remain separate. `affected_files_total` counts represented documents, not every
potentially affected file in the program. `file_summary_scope=returned_symbols`
and `file_summary_complete_for_returned_symbols=true` explicitly restrict these
counts to the returned bounded symbol list. An item/depth cap can omit further
files and symbols. `program_impact_complete=false` applies even when this graph's
bounded traversal is exhausted. These static candidates are not stated execution
facts or calibrated confidence counts. No source bodies are added.

Invalid source documents (stale, unindexed or parse-error) are excluded from
symbol traversal roots and intermediate nodes. Diagnostics and excluded-root
counts remain visible; graph completeness never proves program impact.

## SQL table targets

Set `impact_table` to a SQL identifier and optionally `impact_project` (exact
wing/project), `impact_table_mode` (all/read/write) and `path` (direct SQL source
filter). `symbol` cannot be combined with a table target. SQL quoting/case and
qualified identifiers use the same canonical parser as table_usage; trailing SQL
is rejected. Unqualified identifiers match qualified leaf names, but matching
multiple project/table identities returns needs_disambiguation with a bounded
candidate list and no traversal. A qualified target does not guess a schema for
unqualified SQL mentions.

`direct_readers_writers` preserves static table-use evidence, including raw SQL
without a symbol owner. Its limit/total/truncated are independent of indirect
items. All matched valid owner symbols seed the incoming graph even when direct
evidence is truncated. Their depth is zero; returned dependent symbols begin at
one. Direct owners are not repeated as indirect results. Existing relation modes,
project identity guards and traversal bounds apply. The file summary covers
returned indirect symbols; direct file locations remain in direct evidence.

Ownerless SQL cannot seed symbol dependencies. Names are static mentions, not
proof of a physical table, executed query, row effect or runtime impact. Diagnostic
pages are scoped to project/path, not claimed to be table-specific. Stale sources
are omitted with the usual diagnostics. No source SQL or database connection is
executed. Index format 29 and schema 25 remain unchanged.

## File targets

`impact_file` selects an exact indexed path. `impact_document_id` can select or
disambiguate an exact document; `impact_project` optionally restricts the target.
Multiple matches return a bounded candidate page and no traversal. Mixing file
selection with symbol/path/table arguments is rejected. No live path is opened.

File mode returns `result_kind=files`, with document/path/project, depth and an
original relation witness for each returned file. Its default relation mode is
references: cross-document bound occurrences include imports/value uses without
a function owner. Calls mode restricts edges to calls; all also includes class
inheritance/member candidates. Equal project identity is required on every edge.
Self-file edges are omitted. BFS retains one shortest discovered witness per
document and excludes target files from affected results, with the same depth
and item limits. A file-level chain is a conservative dependency path, not proof
that the same function/value flows through the entire chain.

Stale, unindexed and parse-error documents cannot be targets or intermediate
bridges in this graph. target_found means a matching indexed document record;
target_available means it participates in this fresh captured graph. Neither
asserts filesystem existence or unchanged live bytes. Missing/unavailable targets
keep traversal_complete=false and absence_proven=false. Unsupported loader/import
forms, dynamic/generated/resource links, SQL migration-to-query dependencies
and runtime effects are not inferred. SQL table mode handles named SQL usages
separately. Initial file-impact extraction used format 29; file-import extraction
now uses format 34. Database schema 25 is unchanged.

Static Python/JS/TS module import candidates are now included in file references/all mode independently of symbol binding. See [file imports](FILE_IMPORT_RELATIONS.md) for scope, ambiguity and history semantics.

HTML/CSS local resource candidates additionally participate in file references/all mode. See [resource contract](FILE_RESOURCE_RELATIONS.md) for base/deployment/encoding guards, nonindexable-asset gaps and frozen diff availability.

## Literal key and URL targets

`impact_key` and `impact_route` add captured literal-target impact with separate
multi-root code/file traversals. Project identity is disambiguated before direct
source/route-kind filters; output caps do not truncate seed sets. Registered
handler alternatives do not become source owners. Shared symbol nodes provide
`impact_target` selectors. See [contract](LITERAL_TARGET_IMPACT.md).

SQL identity disambiguation also now includes schema-only candidates before
source/access filtering, consistently with references and shared symbol inventory.
