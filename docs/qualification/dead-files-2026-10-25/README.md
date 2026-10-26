# File candidates and literal refutation — 2026-10-25

[Contract](../../DEAD_FILES.md). RAGRAF dead_code_report/literal_refs at pinned
075caf86dc2f6274b9c48487e521915bcdfeaf2c combines graph observations with literal
path checks and reports unexamined candidates. This transfers that file-level
surface into a separate local files view. It uses existing bound/structural facts,
not complete module/resource/runtime resolution. No deletion safety is claimed.

Four native regressions cover preserved HTML refutations/body exclusion and
suffix boundaries; incoming links/shebang protection with stale-project unknowns;
partial positive refutations and byte/document/candidate budgets without false
negative classification; and project/ambiguous-basename separation plus relative
path evidence and stable reads. Existing routine reachability cases also run.
The actual MCP fixture checks those contracts through source sync and ingestion,
using owned temporary files/databases and mock providers. No source app executes.

The literal scan limits exclude the existing graph validation and metadata phase;
no total CPU/RSS/IO budget is claimed. All phases use a single read transaction.
Original HTML without preservation cannot establish absence. Unresolved imports,
path aliases, escaped/generated/runtime links, representative scale/precision and
full RAGRAF parity remain open. Versions, logs and hashes: verification.json;
actual stdio evidence: runtime.json.

Logged gates: **1,286 workspace tests passed, 0 failed, 1 ignored**. Four new
file cases and four routine cases passed. Clippy with `-D warnings` passed. Three
stdio MCP scenarios passed against an isolated executable copy with a checked
hash: files, routine reachability and file impact. Actual runtime versions are
store schema 25 and extractor format 33.

The shared checkout contains active parallel Rust build-context changes; it was
not frozen. Test counts describe the retained build logs. Owned source formatting
and git diff --check passed; the whole-workspace format check still reports the
parallel changes (workspace-format.log). No complete-current-worktree format gate
is claimed. Source/binary hashes and logs are retained here.
