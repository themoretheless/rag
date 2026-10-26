# Possible routine reachability — 2026-10-25

[Contract](../../DEAD_CODE_REACHABILITY.md). The previous local query used a
corpus-wide lexical short-name exclusion and did not propagate that uncertainty
to callees or inherited method candidates. This extends possible routine liveness
without promoting candidates to runtime proofs. RAGRAF op_dead_code at pinned
075caf86dc2f6274b9c48487e521915bcdfeaf2c also handles file candidates and literal
path refutation; those file-level contracts remain open. This is not full
reference dead-code parity or a representative precision/recall comparison.

Four new native regressions cover bound IDs versus unrelated same-name definitions
and other-project unknowns; same-project unresolved value/call roots protecting
callees; base-value reachability through inherited method candidates and their
callees without fabricated calls; and stable capped candidate pages. Existing
analysis/route/dead-code regressions also run. The actual stdio MCP fixture checks
those exclusions, no deletion/dispatch proof, warm reuse, stale omission and
restored candidates after removing a bound root through source synchronization.
Mock providers and owned temporary sources/databases isolate the checks.

No source application executes or code is deleted. Dynamic/external/framework
usage, language dispatch outside the inherited-candidate subset and representative
quality/scale remain unverified. Full RAGRAF parity remains open. The query adds
no schema/extractor migration; actual qualification versions are in runtime.json
and verification.json.

Final gates: **1,267 workspace tests passed, 0 failed, 1 ignored**. Four new
reachability and eight existing analysis cases passed. Clippy with `-D warnings`,
formatting and `git diff --check` passed. The isolated binary passed new dead-code,
existing inherited-method and multi-file impact MCP scenarios. Actual runtime
versions: database schema 25, extractor format 31.

The shared workspace also had parallel Rust-chain changes; test counts belong to
the logged build, not a claim that all parallel work was frozen. Runtime used a
private executable copy with a checked hash. A module-name collision in the first
MCP fixture bound what was intended to be an unknown receiver; the final fixture
uses a distinct receiver name and exercises the unresolved-name path. Compiler-level
receiver/module binding precision is outside this qualification. Logs, source and
runtime binary hashes are retained here. Full reference dead-code/file-refutation
and representative quality/scale qualification remain open.
