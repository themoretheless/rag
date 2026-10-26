# Impact of captured translation keys and URL literals

`impact(impact_key="reg.months", impact_project="web")` and
`impact(impact_route="/items", impact_project="web")` now connect literal target
facts to separate incoming code-symbol and cross-document dependency traversals.
`symbol` shared nodes expose `impact_target` selectors for direct navigation.
No translator, template engine, router or runtime dispatch is inferred.

## Direct facts and identity

Keys use the existing [captured key contract](TRANSLATION_KEY_REFERENCES.md).
URL selectors accept up to 4,096 UTF-8 bytes, aligned with endpoint capture and
references so a captured 3,001-byte endpoint remains navigable. They use exact
captured literal text, with `impact_route_kind=all` (default),
`registration` or `endpoint`. Registration methods and explicit client request
methods stay distinct. Handler alternatives at one registration/method count as
one occurrence and are retained separately, capped at 50 IDs with full totals.
They never become source owners or code seeds simply because the URL matches.

Project ambiguity is resolved before source `path` and route-kind filters. A
filter cannot silently hide another project observing the same literal. `path`
then filters exact direct-source paths. Key/URL, symbol, table and file/document
selectors are mutually exclusive; file path + document ID remain a valid pair.
Misplaced/invalid modes and malformed literal targets return errors.

`direct_occurrences` retains source document/project/revision/hash and offsets,
with `direct_total` and independent truncation. PHP key calls retain captured
lexical owners; raw placeholders retain file origins. A URL source owner is the
smallest captured enclosing definition containing its source span. A decorator
outside its handler definition therefore does not manufacture that handler as
its owner. `owner_resolution` states the basis. These are syntactic source
ownership candidates, not proof of runtime key/URL binding.

## Two dependency graphs

All qualified direct owner symbols seed `code_impact`; all direct source document
IDs (including ownerless templates) seed `file_impact`. Root sets are formed
before output limits. The file traversal is one deterministic multi-root BFS,
not repeated single-file queries. Candidate code/file results are separate.
Top-level `items` and `affected_files` retain the code-result convention; the
nested file graph includes raw-template/resource/import dependents.

New literal modes default both graphs to `impact_relations=references`:
`calls` restricts indirect edges to calls; `references` also includes bound
value/type and file import/resource candidates; `all` adds inheritance/method
candidates. Direct literal facts remain visible in all modes. Existing ordinary
symbol/table modes retain their established defaults.

Direct owners/files are depth zero. Indirect results start at one, with depth
clamped to 1..10, result limit 1..500 and shortest discovered witnesses. Direct
proofs, direct-file pages, code output and file output each have independent
windows; limiting a proof page cannot delete unseen graph roots. `truncated` and
`depth_limited` combine child states. A complete bounded graph traversal is not
complete program/runtime impact; `program_impact_complete` and runtime/absence
proof remain false. Key capture coverage reports incomplete syntax captures.

## Invalid inputs and SQL consistency

The common code impact walker now excludes stale, unindexed and parse-error
source documents as roots and intermediate nodes, across calls/references/all.
Excluded root counts and input diagnostics remain visible. This closes a
reproduced bug where a partially parsed JS module bridged an otherwise valid
client to another caller despite its syntax error. The existing file walker
already enforced that boundary; explicit multi-root traversal preserves it.

SQL table impact now considers both captured schema and use identities before
source/access filters. A schema-only competing table cannot be hidden by choosing
a read/write mode or one source file. A qualified schema-only target can be
`target_observed` with zero direct uses. SQL direct facts from invalid host-source
inputs are excluded, without claiming physical database binding or runtime absence.

No source bodies are returned. No ingested source code or SQL executes. Module-level code outside
captured owners, unsupported imports, runtime aliases/dispatch, unassigned-project
selection, representative precision/recall/scale and whole-query work/memory
budgets remain open. [Qualification](qualification/literal-impact-2026-10-26/README.md).
