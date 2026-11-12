# Independent Rust graph quality protocol

Latest bounded result (qualified format 81): **502/570** positive correspondences, zero
observed false local targets, unchanged 613 external negatives and exact
native/MCP candidate parity at all 1183 sites. The standard arch type milestone
recovers 24 memchr wrapper calls with no lost correct sites; 68 positives
remain unresolved. Evidence:
[standard arch qualification](qualification/rust-stdarch-types-2026-10-26/README.md).
Historical milestone numbers below retain their original scope.

The fixed itoa/ryu/memchr corpus uses independently generated rust-analyzer
SCIP/CST source-definition labels. Original contract 1 retains 570 positive
callable-reference sites; the raw-pointer milestone resolves 459 with zero
observed false targets. Corpus sources, build configuration, RA errors,
exclusions and disabled-branch denominator are recorded in qualification reports.
This is a small language-server correspondence corpus, not compiler/runtime or
representative whole-program proof.

## External negatives: opt-in contract 2

`qualify_rust_corpus.py ... --external-descriptors` admits additional external
Cargo symbols without kind metadata only when a complete valid SCIP symbol ends
in a Method descriptor and the independent CST marks that occurrence as a direct
call. It requires no local definition and an unambiguous occurrence anchor.
Fine-grained Kind remains unknown; kinds=[] is preserved. Existing known-kind
filters remain authoritative. Local missing definitions, escaped text resembling
a method, imports, macro tokens and non-call syntax do not become negative calls.
The default protocol and original labels remain unchanged.

The expanded corpus adds 613 core/alloc negatives. There are zero false local
candidate sites, 608 extracted observations and five missing observations. A
negative abstention is not an extracted/resolved external call or a new positive
hit. Always report positive recall and negative false-binding/extraction counts
separately; pooled exact-site rate must not be advertised as positive recall.

`summarize_rust_external_negatives.py` verifies the final adapter's oracle against
captured outputs, original per-case labels/observations/scores, source/config/SCIP
identity, one frozen server/helper pair and native/MCP parity. All authoritative
baseline artifacts are hashed. It rejects duplicate/ambiguous anchors and creates
its output exclusively. Runtime qualification stores are disposable; never open
the production database through an independent native reader.

Evidence and reproduction:
[external-negative qualification](qualification/rust-external-negatives-2026-10-26/README.md).
Broader repositories, independently checked external dependencies and compiler
semantics, the 111 original unresolved sites and equal-work release competitor
speed/RSS remain open; this protocol expansion does not complete requirement 4
or the overall plan.


The subsequent union-field milestone preserves all 570 positive and 613 external
negative labels while improving local targets from 459 to 465, with zero observed
false local targets. See
[union qualification](qualification/rust-union-fields-2026-10-26/README.md).
This changes resolver results, not the oracle or positive denominator.


## Same-release quality gate

The frozen performance candidate now passes the independent corpus itself:
465/570 local targets, zero false local targets across those positives and 613
core/alloc negatives, exact native/MCP candidate parity at all 1183 sites.
Its guarded native exporter links the server's original optimized Cargo libraries
and full metadata with the same compiler and explicit release options, avoiding
an example/dev-dependency feature rebuild. Library/source/binary identities and
per-case debug-baseline target positions/scores are checked. Two failed partial-
metadata link attempts remain rejected evidence. This qualifies the bounded
same-release quality gate; 105 positive misses and the broader corpus/compiler
limitations remain. Evidence:
[same-release corpus](qualification/same-release-rust-quality-2026-10-26/README.md).
