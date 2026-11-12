# Python receiver-origin join qualification — 2026-10-26

Scope capture 5 / extraction 89 adds bounded local reaching-origin joins to Python member binding. Ordinary live branches, loop fixed points/zero paths, break/continue exits and local aliases now feed the existing scoped import/type/inheritance resolver. Target unions retain unknown alternatives explicitly, including a single known target marked ambiguous. See [contract](../../PYTHON_RECEIVER_JOINS.md).

`before.log` retains the failed optional-assignment regression: the observed targets included the constructor and only the new receiver method, omitting the old method. The final assertion isolates method targets and requires both. Known straight-line resolution bases remain compatible; inherited duplicate body targets collapse to one candidate.

Dead-code unknown-name roots and affected-test uncertainty accounting now include partial-known references. Sparse resolver overrides avoid copying full value/source payloads for each origin. Missing flow marker/inventory payloads do not qualify as current captures. Value capture/normalization remain 7/3.

Final validation:

- `cargo test --offline --workspace`: **1700 passed, 0 failed, 1 ignored**, `workspace-final5.log`.
- Nine added native regressions cover optional/both-arm joins, returned-arm exclusion, zero/backedge/break/continue loop values, aliases/partial unknowns with dead-code protection, imported/inherited target deduplication/project isolation, origin budget fallback, last import alias, unknown destructured writes and missing flow inventory guards. Some tests bundle parallel cases.
- Strict workspace/all-target Clippy passed, `clippy-final4.log`.
- Binary build passed, `build-final4.log`.
- **Thirteen real stdio MCP scripts passed** on one copied executable, `verification.json`: receiver joins, assignment order, handler routes, finally routes, frozen value diff, value slices, Python binding, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, contextual PHP trace and Git impact.
- Receiver-join MCP passed **16 named checks**, `python_receiver_joins.json`: branch unions/kills/returned arms, loop fixed points and zero paths, local NFKC aliases, partial-known uncertainty, imported inherited-body deduplication, unknown-name dead-code guard, impact from both targets, exact bytes, project isolation, warm/full resolution equivalence, capability proof limits, stale exclusion and source-sync target refresh. The dead-code fixture includes an otherwise unreferenced fourth method.
- Prior assignment/handler/finally/value-slice/value-diff MCP retained **15/20/20/35/12 checks**; SQL impact retained **15**.

Final binary SHA256: `0a60efaa23c5d6a7a79489730d386deb5610821d9c9929d4ee3e658dbe9d5009`.
`verification.json` records exact totals, thirteen MCP exit codes and selected source/dependency hashes before/after MCP. Selected files and the copied executable were unchanged; the shared tree was not frozen. `run_mcp.py` repeats the MCP batch from the repository root. `verification-initial.json` preserves the earlier passing batch before the missing-inventory guard.

Earlier logs remain: the initial flow module had a Result-alias compile error; an older reference test exposed unnecessary resolution-basis churn, corrected by preserving single-origin bases. A repeated-import regression exposed the need to select the last same-statement binding. Later runs cover those fixes, explicit origin exhaustion, destructured unknown writes and the cache inventory guard. No failed scenario was dropped to obtain final success.

MCP uses temporary databases, mock embeddings and disabled LLMs. Analyzed Python/SQL is never executed; production data was untouched. These are captured-model correctness/integration cases, not runtime dispatch, representative corpus or performance superiority proof.

Still open: precise exceptional/closure/default receiver environments, unvisited/unsupported syntax and full reachability, general destructuring/heap/alias inference, runtime subtype contexts, complete compiler/CFG/CPG semantics, and broader corpus/language/scale acceptance. Logical capture bounds are not physical memory/time/full-response byte qualification. Every candidate remains syntactic and runtime-unproven.
