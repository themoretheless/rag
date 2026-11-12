# Frozen Python value-graph comparison

`diff` adds `data_flow_comparison` for compatible format-5/6/7 frozen Python value captures. Normalization format 3 compares the immutable source blobs, captured facts and frozen call candidates from the requested index revisions. It never consults the live filesystem or runs analyzed code. No extraction version bump is required: existing compatible format-5/6/7 revisions contain the inputs.

```json
{"before":12,"after":18,"project":"alpha","limit":20}
```

The report contains `nodes_changed`, `edges_added` and `edges_removed` with full totals and independently bounded collections. Node changes distinguish added/removed/changed; edge records retain relation and normalized caller-frame identity. Evidence includes document/project/path/revision/content hash, function identity and actual occurrence byte ranges. Source bodies and literal text are not included. `syntax_hash` fingerprints syntax, not computed values.

Normalized owner identity combines document ID, stored project, NFKC-qualified function name and symbol kind. Node identity combines that owner, captured kind, normalized local binding or synthetic merge/header slot, captured completion context, and occurrence ordinal within that group. Absolute offsets and hashes of whole source files are evidence rather than identity. AST fingerprints include operators, literal spellings and child syntax, normalize identifier keys and omit comments. Ordinary spacing/comments and NFKC-equivalent identifiers therefore do not create position-only changes. Literal/operator changes can change node fingerprints without changing topology.

This is not a semantic or minimal-edit matcher: redundant parentheses, literal quotation/escape forms, line-continuation syntax, reordered statements or inserted repeated occurrences can produce changes. Occurrence ordinals can shift when similar syntax is inserted/deleted. Renamed functions are compared as removed/added owners. Duplicate normalized function declarations are excluded rather than paired arbitrarily. Identical paths in different documents/projects remain distinct.

Local edges include branch/loop control and cyclic reaching-definition candidates. Uniquely qualified, same-project, matching-arity positional calls add `call_argument` and `return_to_call` edges using the exact frozen call site. There is no generic argument-to-result shortcut or runtime dispatch inference. Import-only redirection can change those edges even if function-body syntax is unchanged.

`available` means the compatible comparison algorithm could run. `complete_for_captured_scope` additionally requires comparable documents/functions and no unverified call boundaries; it does not mean whole-program completeness. Documents with missing, incomplete, parse-error or mismatched capture are excluded on both sides. A function unsupported on either side is excluded on both sides. These gaps are reported with evidence instead of emitting fictitious edge deletions. Actual function/document removal can remove previously captured facts; it still does not prove runtime absence. Corrupt frozen edge/call ownership/provenance makes comparison unavailable. Legacy or unknown capture formats are unavailable and omit change totals rather than inventing zero changes.

The query's existing `limit` bounds each result and exclusion collection. Exact totals are retained; any clipped collection sets comparison `truncated` and propagates to outer `diff.truncated`. Normalization has a shared 200000-work limit across both sides, counting scoped AST visits/fingerprints, symbols, facts and relevant frozen call bindings. Loading/decoding stored source and graph JSON, scanning scope metadata, assembling maps/differences and response formatting are separate; this is not a wall-time or memory bound. Exhaustion is explicit as an exclusion or unavailable report and cannot authorize absence. Large-corpus performance and full-response byte budgets remain unqualified.

`source_semantic_change_proven`, `runtime_path_proven` and `absence_proven` remain false. The report compares captured syntactic candidates, including unused values; it is not a diff of one particular `data_slice` query. Complete CFG/exception/heap/alias semantics, predicate/iteration feasibility, termination, broader languages and full CPG acceptance remain open.

`index_status.capabilities.frozen_value_diff` advertises the algorithm, formats and proof limits. Per-revision coverage must still be read from each diff response.

[Qualification](qualification/python-value-diff-2026-10-26/README.md).

Capture-6 completion contexts and legacy-5 compatibility: [contract](PYTHON_FINALLY_FLOW.md), [qualification](qualification/python-finally-flow-2026-10-26/README.md).

Normalization 3 excludes context-bearing functions across different capture formats; their inventory/analysis-model changes are not emitted as source-edge deletions. Same-format handler, else and finally contexts are supported; see [exception routes](PYTHON_EXCEPTION_FLOW.md).
