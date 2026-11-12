# Python assignment-order qualification — 2026-10-26

Fixed a reproduced binding error: `receiver = New(receiver.work())` previously selected `New::work` for the RHS call even when the preceding parameter type was `Old`. `before.log` retains that failing regression and its exact source evidence. Deferred assignment activation now preserves `Old::work` on the RHS and selects the captured `New` type only after the assignment boundary. See [contract](../../PYTHON_ASSIGNMENT_ORDER.md).

Scope format 4 / extraction 88 additionally qualifies first-write uninitialized guards, ordinary module/class RHS global lookup, annotation-only local classification without replacing initialized values, conservative augmented-result invalidation, chained assignments and NFKC spellings. Public source evidence is unchanged in shape. Value capture/normalization remain 7/3.

Final validation:

- `cargo test --offline --workspace`: **1691 passed, 0 failed, 1 ignored**, `workspace.log`.
- Six new native assignment/annotation/augmentation regressions; the first focused run passed all 11 then-current Python scope tests (`focused.log`), followed by the final workspace with the additional cases.
- Strict workspace/all-target Clippy passed, `clippy.log`.
- Binary build passed, `build.log`.
- **Twelve real stdio MCP scripts passed** on one copied executable, `verification.json`: assignment binding order, handler paths, finally paths, frozen value diff, value slices, Python name binding, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, contextual PHP trace and Git impact.
- New assignment-order MCP passed **15 named checks**, `python_assignment_order.json`: preceding/new receiver targets, first-write guard, annotation-only scope and retained value, unknown augmented result, chained NFKC writes, module/class global RHS, exact bytes, impact consistency, project isolation, warm/full resolution equivalence, advertised proof limits, stale exclusion and source-sync annotation refresh.
- Existing handler/finally/value-slice/value-diff MCP retained **20/20/35/12 passing checks**; SQL impact retained **15**.

Binary SHA256: `445b0e7c37a39c17ea7b641a05a4aa0096de86ebe5905bc257b199ec3dcaf225`.
`verification.json` records exact totals, twelve MCP exit codes and selected source/dependency hashes before/after MCP. The executable and selected files were unchanged; the shared tree was not frozen. `run_mcp.py` repeats the MCP batch from the repository root.

All MCP fixtures use temporary databases, mock embeddings and disabled LLMs. Analyzed Python/SQL is never executed; production data was untouched. These are source-binding correctness cases, not compiler/runtime, representative corpus or old-binary performance proof.

Still open: branch-sensitive receiver type unions, destructuring and heap/alias effects, complete type/operator/exception/class matching, whole compiler/CFG/CPG semantics, representative precision/recall/scale and full RAGRAF parity. Static post-assignment type candidates do not prove successful initialization or reachability. Full-response byte/memory/time budgets remain unqualified.
