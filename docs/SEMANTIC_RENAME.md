# Captured Rust semantic rename

`rename` is a read-authorized preview. Required fields are `symbol`, `new_name`
and `project`; `path` disambiguates a definition. An exact symbol ID takes
precedence over coincident names. `apply_rename` is a separate **admin** operation
with `{query: <same selector>, plan_token: <reviewed preview token>}`. The preview
schema rejects application flags, and write-only callers cannot apply a rename.
Both tools require the full MCP surface and the configured source allowlist.

The preview joins fresh captured normal/test-cfg bindings with exact source AST
name anchors. Initial selectable definitions are extracted Rust functions,
structs, enums and type aliases. Constants/statics are not currently extracted
as selectable symbols. It emits original UTF-8 byte ranges and old/
new text, declaration or binding evidence, document identities, and source hash
guards. Comments and strings are not textually rewritten. Raw names are compared
without their prefix and with Unicode NFC normalization; reserved names and
syntactically invalid identifiers are refused.

Every matching identifier must be an exact edit, a separately bound other target
or a separately identified unrelated declaration. Unknown occurrences block
application. A use of the proposed new identifier anywhere in captured Rust
source blocks as a possible collision; this is intentionally broader than
compiler scope collision analysis. Multiple candidate targets, alias spellings,
trait method families, unsupported definition/source languages, missing/stale/
unindexed/parse-error inputs and binding diagnostics also block. Module/file
moves, macro expansion, generated code, inactive alternate configurations and
external consumers are not a compiler-complete rename implementation. Inactive
or macro identifier occurrences that cannot be accounted for block the plan.
Qualified intermediate path segments that lack an exact bound occurrence also
block; declaring a supported kind does not imply all its Rust use forms work.
The test overlay admits captured cfg(test) references; it does not model every
possible Cargo compilation unit or feature set.

Recognized documentation/configuration inputs (Markdown, YAML, JSON, TOML,
lock/log files, Dockerfile/Containerfile variants and selected ignore files) stay
under the same live hash guards but do not become Rust identifier/collision
sources. Their text is not rewritten. Other source-code languages and unknown
input formats still block. Documentation examples, generated code and string-
based configuration consumers require manual review and compiler/test checks.

All immutable indexed files for the selected project, including provider and
manifest inputs that receive no edit, are read from allowlisted regular files
and compared with the recorded source hashes. Symlink final files, multiply
linked files, BOM/non-UTF-8 sources and files above 4 MiB are refused. Limits are
1000 captured files, 32 MiB source, 10000 edits and 4 MiB preview JSON; overflows
fail without an applicable partial token. No filesystem walk discovers omitted
or new files, so these checks establish captured-input freshness, not complete
repository coverage. Output explicitly retains `coverage_complete=false` and
`compiler_verified=false`.

The token hashes a deterministic preview. Apply recomputes the complete plan
under the Store lock and requires an exact match. It rechecks all live guards,
validates edited Rust syntax, writes same-directory exclusive `.new` files and
original `.bak` copies with source permissions, syncs them and records a
`.rag-rename-<uuid>.json` journal before replacing sources. Each replacement and
journal update is synced on Unix. Backups and the journal remain after success.
The index is not silently updated: the result requires re-ingestion/indexing and
compiler/test checks against the changed sources.

Ordinary replacement/journal failures attempt reverse-order rollback. A source
whose current hash differs from the applied content is preserved as a detected
concurrent change, and the response reports `recovery_required` with retained
backup/journal paths. Successful rollback reports `rolled_back`; both outcomes
set `success=false`. Staging failures can leave exclusive temporary/backup files
but do not change source files. Do not remove recovery artifacts until the user
has accepted the result or restoration is complete.

This is **not** a crash-atomic multi-file filesystem transaction. A crash can
leave a partially applied set; journal files list old/new hashes and originals
for recovery, including the replacement-before-journal-update window. No
automatic crash-recovery command is implemented yet. Hash checks cannot prevent
an uncooperative editor from writing in the final check/replace interval; pause
other source writers during application. Windows replacement/directory durability
has not been qualified. No general compiler resolution or external-reference
completeness claim follows from syntactic success.

Qualification and exact outcomes:
[semantic rename evidence](qualification/semantic-rename-2026-10-26/README.md).
Requirement 9 remains bounded by the unsupported cases above; the complete
original plan remains active.
