# Search and context packing budgets

`search.max_context_tokens` and `pack_context.max_tokens` limit the complete
citation block: citation headers, inter-hit separators, and retained source text.
Neighbor and parent-section expansion consume this same budget. HTTP
`POST /v1/pack-context` and MCP `pack_context` share the implementation.

The estimator is `ceil(Unicode scalar count / 4)`. A returned `context_text`
satisfies `total_tokens <= max_tokens` under that estimator. This is an estimate,
not an exact count for a particular language-model tokenizer; callers that need
an exact model-window limit should tokenize the final prompt, including their
instructions and other messages.

Hits retain rank order. The final retained body can be shortened with an
ellipsis. If a citation header and one body character cannot fit, that hit and
the following hits are omitted. A zero budget returns no hits and an empty
block. A very small positive budget can therefore also return no hits.

Packed `hit.content` is the canonical body, including any requested expansion
that fits. Duplicate `hit.context` and `hit.snippet` fields are omitted, so they
cannot retain full source text outside the budget. Ranking and provenance fields
remain available. The budget applies to the prompt block, not the JSON envelope
or the size of metadata. A direct library search with
`SearchQuery.max_context_tokens = None` is unbudgeted and retains its normal
snippet/context fields. HTTP/MCP searches use the configured budget when the
parameter is omitted.

This tightens the previous behavior: citation headers used to sit outside the
budget, expanded context could survive in auxiliary fields, and zero disabled
packing in search. Existing callers may receive shorter results for the same
number. Increase the explicit budget if more source text is needed; consume
`context_text` from `pack_context` directly when constructing a prompt.

## Document diversity and candidate refill

Search starts with `max(50, top_k * 5)` candidates per retrieval source. If a
document cap leaves fewer than `top_k` eligible hits while a source may still
have candidates, search doubles the pool, up to 4,096 candidates per source.
Vector, lexical, and hybrid searches retain their original project and other
scope filters on every round. A request scoped to one document does not refill
to try to satisfy an impossible multi-document quota.

The original request deadline applies across all rounds. Vector and lexical
timings accumulate the work from every round; ranking boosts, context expansion,
and prompt packing run once after refill. A small prompt budget never triggers
extra retrieval. Result explanations include `diversity_candidate_refill` when
refill was needed and `diversity_candidate_cap_reached` when the bound prevented
further refill. The latter can mean fewer than `top_k` results even if deeper
candidates exist; use a narrower scope or a larger per-document allowance.
