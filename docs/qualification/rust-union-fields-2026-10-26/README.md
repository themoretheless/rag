# Rust declared union field qualification

Bounded receiver/configuration milestone; full code-intelligence plan remains open.

## Controlled quality results

| Repository | Positive labels | Before correct | After correct | External negatives | False local target sites after |
|---|---:|---:|---:|---:|---:|
| itoa | 9 | 9 | 9 | 48 | 0 |
| ryu | 95 | 93 | 93 | 142 | 0 |
| memchr | 466 | 357 | 363 | 423 | 0 |
| Combined | 570 | 459 | 465 | 613 | 0 |

All six recovered sites are memchr direct calls through declared union fields.
No previously correct target is lost. Positive local recall improves 459/570 to
465/570 (81.58%); external negative abstentions are not positive hits. All 613
external sites remain without local candidates; 608 have extracted observations
and five lack observations. The five missing observations are not successful
extraction or external resolution. Pooled exact-site rate is not positive recall.

The baseline is the preceding external-negative milestone. Source bytes, captured
Cargo/config inputs, independently derived SCIP/CST oracle and original/expanded
labels are unchanged. The summarizer recomputes scores and validates input,
oracle/label identity, one frozen server/helper pair per phase and exact
native/actual-MCP parity for all 1183 labelled sites. Baseline artifacts were
byte-checked and hashed. Transitions retain original ranges and before/after
observations. No source or denominator rewrite improves the score.

Original language-server errors, excluded ambiguous/generated anchors and
configuration limits remain. This is a small three-library source-definition
correspondence corpus, not universal/compiler/runtime precision. External
negatives cover core/alloc, not broad third-party dependencies.

## Implemented behavior

Formats 69/70 add union nominal definition identities, generic type parameter
facts, named fields, type-scope behavior and configured owner/member filtering.
Existing exact-owner and source-anchor rules govern unknown/duplicate alternatives.
Same-name union type and function value declarations stay distinct: a union is
not a callable value constructor or a candidate that makes a function ambiguous.
Generic and nested declared field types propagate to static method candidates.
Union definitions remain explicitly unsupported by semantic rename.

Three new semantic native tests cover cross-file imported generic unions, nested
fields, distinct method owners, missing fields and same-name function/type
namespaces. A fourth cfg test covers unknown competing fields in both source
orders; the existing feature-selection test now covers both conditional union
fields and conditional same-name union owners. Negative extraction snippets may
be compiler-invalid; they are not presented as compiler-valid programs.

The compiler-valid fixture selects fast/slow through captured Cargo metadata.
Actual MCP checks eight exact targets, excludes stale edges before refresh,
reindexes only the changed manifest, reuses warm no-op revisions and observes
frozen edge changes. Cargo executes one passing test per feature configuration,
reading only initialized union fields and checking distinct Fast/Slow values.
The fixture also compiler-checks a union and function sharing the name Slot.

Declared field types do not establish active-member choice, initialization,
unsafe validity, layout, aliasing or observed dispatch. Complete union/compiler
and flow-sensitive active-member semantics remain open.

## Executables and validation

Final frozen server SHA-256:
edb5c3ea69692f46690b1b974f98bf4c9f8c284d4a04b0976ecf830efe41af55.
Native helper SHA-256:
1539325c105a95c35421b8e8fec03c75b6d55f5aea67ffd7271ed3ed09d08c40.
Index format 70. Final corpus and runtime runs use this pair. Earlier format69
experiments remain separate temporary artifacts and are not final evidence.
Focused Rust: 84 passed. Strict Clippy and five Python protocol/metric tests pass.
Final workspace: 1528 passed, zero failed, one ignored. Strict Clippy also
passed on the updated workspace. checks.json and logs retain the results.

A preliminary full workspace run failed in the concurrently changed code-cursor
rechunk test; its log and isolated failed recheck are retained. The updated
worktree's isolated recheck subsequently passed before the final full rerun.
No code_read implementation or test changes are attributed to this union work.

The source checkout remains shared/dirty and not frozen. Runtime binaries were
frozen before final corpus/fixture runs. Production executable unchanged;
qualification stores are exclusively owned and disposable. No release-speed,
RSS or competitor-superiority claim follows. Remaining 105 positive misses,
broader compiler/build/generic semantics, third-party negative quality and
comparative release performance keep the original full goal open.
