# Git source change impact

`detect_changes` captures two source states, builds native ephemeral code graphs,
maps structural changes and returns incoming dependency witnesses on both sides.
It uses the same parsers, Cargo/cfg/receiver/trait binding and impact traversal as
the persistent index. It does not require source ingestion or assume that an
existing stored index matches HEAD, the index or the working tree.

```json
{
  "repository": "/absolute/allowlisted/repository",
  "scope": "staged",
  "project": "my-project",
  "relations": "calls",
  "depth": 3,
  "limit": 100,
  "max_affected": 100,
  "max_seeds": 500
}
```

Scopes:

| Scope | Before | After |
|---|---|---|
| `working_tree` (default) | Git index | Actual supported working sources, including untracked nonignored sources |
| `staged` | HEAD | Git index; unstaged content is excluded |
| `all` | HEAD | Actual supported working sources, including staged and untracked changes |
| `commits` | Required `before` commit reference | Required `after` commit reference |

References are resolved once to commit OIDs. Blob reads then use those pinned
objects, never a checkout. Commit comparisons are independent of later source
edits. Staged/all require an existing HEAD; an unborn repository can use the
index-to-working scope. References starting with `-`, control characters and
invalid references are rejected. The exact repository root must be under
`RAG_INGEST_ROOTS`; a subdirectory is not accepted as a different repository.

Capture invokes only read-only Git builtins (`rev-parse`, `ls-tree`, `ls-files`,
`cat-file`). It never invokes `git diff`, checkout, clean/smudge/textconv filters,
hooks, network commands or index writes. Git redirecting environment variables
are removed; fsmonitor, optional locks and global/system configuration are
disabled for these commands. Ordinary worktrees with external Git object storage
remain supported. The tool does not mutate source or persistent source/index
tables; the MCP framework may still record ordinary call/audit telemetry.

Git index entries/flags and untracked membership are rechecked before return.
Working files use the ingestion decoder, bounded reads, original-byte hashes and
per-file revalidation. Missing tracked regular sources are deletions; absent
skip-worktree sources use their pinned index blobs with explicit projection
evidence. A changed source/index observation aborts with a retryable error. This
is a revalidated capture interval, not an OS-wide atomic filesystem snapshot.
Filesystem symlinks/nonregular sources are not followed for graph extraction;
escaping source ancestors are rejected. Missing/unverified Cargo/go manifest
boundaries remain barriers instead of enabling conventional-layout fallbacks.

`before`/`after` record commit/index/working identity, capture hashes, source
counts and unavailable/unsupported samples. Every changed file carries original
Git blob or live-byte/encoding evidence plus decoded source hashes. Transient
document IDs start with `git:` and document revisions are zero; they are not
stored-document IDs or stored index revisions. `persistent_index_used=false` and
`graph_origin=ephemeral_git_source_snapshots` are explicit. This tool is not
counted by the analysis-task whitelist as proof of a persistent-index query.

Symbols are grouped by repository-relative path, qualified name and syntax kind.
Body hashes identify `modified`; source-independent outgoing bindings identify
`binding_changed`; explicit configuration identity changes identify
`configuration_changed`. Other statuses are `added`, `removed`,
`ambiguous_changed`, `unavailable_before` and `unavailable_after`. Duplicate
groups are never paired by order. Syntax errors and unavailable source sides
must not be reported as proven symbol addition/deletion. Renames across paths
appear as removal/addition; no textual rename pairing is attempted. Container
definitions may change along with their nested methods.

`before_impact` preserves relationships to removed definitions; `after_impact`
shows current captured relationships. Returned changed groups seed one bounded
incoming traversal per side. Each affected symbol has a complete retained
witness to one seed: definitions, typed steps, original occurrence evidence and
the seed's change key. Shared paths keep one shortest discovered witness per
symbol. A symbol that is itself a change seed is listed directly, not repeated
as an indirect impact. Candidate/static relationships retain their uncertainty;
`relations=references` also admits bound values/types and `all` admits
inheritance/override candidates. Runtime impact and absence are never proven.

Limits are explicit and clamped: source captures across both sides default to
2000 files (cap 10000) and 8 MiB (1 KiB..128 MiB), with 4 MiB per file. Source
revalidation adds reads; skip-worktree projection may add object capture work.
Capture overflow refuses the query without publishing a partial graph. Each Git
command has a 30-second deadline and bounded retained output; parsing/resolution
time is separate and is not a strict end-to-end deadline or memory guarantee.
Output defaults to 100 changed file/groups (cap 500), 100 affected symbols per
side (cap 500), 500 change seeds per side (cap 1000) and depth 3 (1..10).
Duplicate-definition samples cap at 20 per group; source/diagnostic/context
samples cap at their reported limits. Totals and truncation are explicit. Seed
truncation and omitted changed groups can omit impact paths. Bounds count
records, not exact response bytes/tokens.

Scope is supported source/manifests. Non-code tracked entries can be compared by
object identity in staged/commit states; unsupported working files are not read
or checked for content changes. Working executable-bit changes are not compared
to Git mode bits. Submodule contents, external dependency sources, runtime
dispatch, full Cargo features/macros and full program/data-flow analysis retain
their existing limits. Source extraction completeness, representative quality,
dependency-scoped cache reuse and competitive performance remain separate work.
Both graphs are rebuilt for this query; no indexing-speed improvement is claimed.

`include_tests=true` enables a transient test-cfg view and attaches per-side
related-test witnesses and command suggestions. See [AFFECTED_TESTS.md](AFFECTED_TESTS.md)
for bounds, source-state requirements and incomplete coverage. This is not a
safe CI test-omission policy.

Qualification: [native and real-MCP Git evidence](qualification/git-impact-2026-10-25/README.md).
