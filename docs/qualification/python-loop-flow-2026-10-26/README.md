# Python loop value-flow qualification — 2026-10-26

Value capture format 5 / extraction format 85 adds bounded cyclic may-dependencies for `while` and plain-target `for`, including `break`, `continue` and `else`. Header values retain entry and backedge definitions; returned paths cannot feed later iterations. Normal exit and break exits are merged separately, preserving the zero-iteration path. `for` records abstract item provenance from its once-evaluated iterable. See [contract](../../PYTHON_DATA_FLOW.md).

Python name-scope capture format 2 adds unknown plain `for` target bindings. They shadow normalized global names in function scope without disabling unrelated static imports. Complex iteration patterns remain explicit guards. Frozen graphs carry `python_names_format=2` and `data_flow_format=5`; legacy facts are repaired and comparison availability remains explicit.

The query now includes checked reachable physical edges rather than only the predecessor-witness forest. Cycles and alternative dependencies remain visible. Each edge records one matched transition direction/from-stack/to-stack context; node witnesses retain their separate seed paths. A fixed 2000-edge output cap is explicit and disables complete-subset claims when hit. Edge unions and source candidates do not prove a feasible execution path.

Final checks:

- `cargo test --offline --workspace`: **1646 passed, 0 failed, 1 ignored**, `workspace.log`. Includes 22 value-flow tests, plain-for binding shadowing, nested jump ownership, once-evaluated iterables, early returns, zero-iteration initialization, format-4 cache repair, cyclic edge visibility and a dense graph that reaches the exact 2000-edge cap.
- `cargo clippy --offline --workspace --all-targets -- -D warnings`: passed, `clippy.log`.
- `cargo build --offline --bin rag-mcp`: passed, `build.log`.
- Eight real stdio MCP scripts passed on one copied executable, `verification.json`: Python value flow, Python bindings, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, PHP contextual trace and Git impact.
- Python value-flow MCP passed **35 named checks**, `python_data_flow.json`. Checks include while/for interprocedural calls, loop entries/backedges, break/continue/else, incomplete initialization, once-evaluated iterables, independent work limits and mechanically matched edge/witness call contexts. Existing fresh/warm/full-resolution/source-sync/frozen-history cases passed.
- SQL table impact retained all **15 passing scenarios**, `table_impact.json`.

Binary SHA256: `b3959021e07c92874217013bb5e44005b0fffdff1390dc434ab981a5626ebed7`.
`verification.json` records selected source/dependency hashes before/after MCP execution. Selected files and the copied executable were unchanged; this shared working tree was not frozen. `run_mcp.py` repeats the MCP batch from the repository root.

Earlier focused logs are retained. `focused.log` contains the obsolete assertion that a while function must be unsupported. `focused2.log` exposed the real predecessor-forest limitation: abstract iteration provenance existed in capture but was absent from returned edges. The query edge-context change fixes that limitation; later focused and workspace tests pass. No fixture expectation was relaxed to accept the missing cyclic/dependency edges.

Fixtures use temporary databases, mock embeddings and disabled LLMs. Analyzed Python and SQL are never executed; production data was untouched. This is not old-binary/runtime superiority evidence.

Still open: complete CFG, exceptions/finally, heap/alias and iterator-protocol effects, predicate/iteration feasibility and correlation, termination, normalized value-history diff, broader languages/corpus/scale and full RAGRAF/CPG parity. Constant-true loops still retain an unproven zero-iteration alternative; individual iterator elements/cardinality are not identified. `runtime_path_proven`, `termination_proven`, `loop_iteration_feasibility_proven`, `full_program_slice` and `absence_proven` remain false.
