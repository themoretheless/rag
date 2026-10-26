# Reviewed Rust rename qualification

This advances requirement 9 of the [complete plan](../../CODE_INTELLIGENCE_PLAN.md).
The [contract](../../SEMANTIC_RENAME.md) documents supported captured Rust forms,
blocking cases, review tokens, filesystem durability and recovery limitations.
It is not whole-compiler rename completeness or whole-plan completion.

## Actual MCP and compiler evidence

`runtime.json` records an actual stdio MCP preview and application of four exact
UTF-8 byte edits in three files: declaration, normal call, callable-value reference
and cfg(test) call. A Unicode comment precedes the declaration. The preview is
deterministic and leaves source files unchanged; wrong tokens and a changed
unedited input are refused before mutation. The original comment/string text is
preserved. Cargo executes one passing test both before and after rename. Original
backups match captured bytes; the applied journal names all three changed files.
The index requires refresh after application, and actual re-ingestion/reindexing
restores the renamed call binding. An application flag on preview is rejected.

`roles.json` tests actual authenticated HTTP MCP discovery and direct dispatch.
Read and write credentials can preview, cannot discover apply and receive
`FORBIDDEN` when invoking it directly. Admin discovers apply; an invalid token
reaches the handler and is refused as an unreviewed plan. No HTTP role test edits
a source file. The actual successful application is exercised through local MCP.

Both reports and the resolution/affected-tests regressions use frozen binary
SHA-256 `abd2c80c7825844abd21321077070e7ddf6a6bb8e3ad04b25e8e0b77a804963e`,
index format 45. This format includes concurrent C# baseline work; rename does
not own or claim that language addition. The source checkout was not frozen.

## Native and regression boundaries

Five native tests cover exact nonmutating preview/application, retained original
backups, explicit refresh, wrong tokens, unedited-input staleness, collisions,
unknown macro occurrences, aliases, trait families, reserved/invalid identifiers,
allowlist/symlink/hardlink/encoding/read-only boundaries and access role mapping.
An injected failure at the second replacement rolls the first file back. When
an external edit changes the first file before rollback, it is preserved and the
result reports `recovery_required`. These injections test the ordinary failure
path; they do not establish crash-atomic behavior or eliminate filesystem races.

The final workspace regression passed 1367 tests, zero failed, one external-DB
fixture test ignored. Strict library/test Clippy and all five final native safety
tests passed. Exact outcomes and artifact hashes are recorded in `checks.json`.
`resolution-regression.json` passes all
four full-binding equivalence/lifecycle cases. `tests-regression.json` retains
the previous seven independently labelled affected tests and all seven commands
actually execute one passing test; `commands.log` retains their output.

## Remaining limits

The positive runtime corpus is a small Rust fixture. It does not measure
representative rename precision/recall. Collisions are conservatively project-wide;
aliases, trait families, macros, some qualified intermediate paths, other source
languages, file moves and alternate build configurations can block. External,
dynamic and generated consumers are outside the captured scope. New files are
not discovered by preview. Compile/test validation is required after applying.

Application retains a recovery journal and original backups and attempts ordinary
rollback. It is not a crash-atomic multi-file transaction or atomic compare-and-
swap against uncooperative editors. Automatic crash recovery and Windows
durability are unqualified. Pause other source writers during application.
All qualification mutations affect disposable sources/stores. The installed
executable was not replaced; no commit or push occurred. Full original quality,
performance/comparison, infrastructure and remaining Cargo/typing work stays open.

## Reproduction

```sh
cargo test --lib code_index::rename -- --nocapture
python3 scripts/smoke_semantic_rename.py /path/to/rag-mcp /tmp/rename.json
python3 scripts/smoke_rename_roles.py /path/to/rag-mcp /tmp/rename-roles.json
```
