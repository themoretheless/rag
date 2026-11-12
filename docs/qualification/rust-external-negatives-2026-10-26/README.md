# Additive independent Rust external-negative qualification

This milestone strengthens requirement 4's quality evidence. It does not improve
positive recall or complete the full code-intelligence plan.

## Results

| Repository | Original positive labels | Correct local targets | Added external negatives | With extracted observation | Without observation | False local candidate sites |
|---|---:|---:|---:|---:|---:|---:|
| itoa | 9 | 9 | 48 | 48 | 0 | 0 |
| ryu | 95 | 93 | 142 | 142 | 0 | 0 |
| memchr | 466 | 357 | 423 | 418 | 5 | 0 |
| Combined | 570 | 459 | 613 | 608 | 5 | 0 |

All 570 original labels, observed definition positions, observation facts and
per-case scores remain exactly unchanged. Local positive recall stays 459/570
(80.53%). The additional 613 sites test false local binding at independently
identified external calls: 611 core and two alloc references. They are not extra
correct local call targets. Pooled exact-site rate includes negative abstentions
and must not replace positive recall or be called overall call-resolution recall.

608 external sites have extracted observations; five have no extracted observation.
A missing observation is not successful call extraction or external resolution.
summary.json lists all five original ranges/symbols in memchr src/vector.rs.
No external site has a local candidate. This is evidence about sysroot negatives
in these three libraries, not broad third-party or universal precision.

## Independent labels and strict admission

The opt-in oracle contract 2 supplements the unchanged default contract 1. Some
rust-analyzer SCIP external symbols omit SymbolInformation.kind. We consume their
complete SCIP symbol grammar, requiring an external rust-analyzer Cargo package,
a terminal Method descriptor and an independently captured CST direct-call anchor.
No fine-grained Function/Method Kind is invented: these labels retain kinds=[].
Escaped identifiers are consumed as a whole, including doubled backticks; ().
inside an escaped term is not accepted as a method. Malformed/parameter/term
suffixes, missing local definitions, non-call/import/macro anchors, conflicting
known kinds, external symbols with captured definitions and ambiguous source
anchors are not admitted by the descriptor fallback.

Grammar source: [SCIP symbol and descriptor specification](https://github.com/sourcegraph/scip/blob/main/scip.proto),
checked 2026-10-26. The Method descriptor is callable evidence here, not a claim
about language-specific fine-grained Kind or runtime dispatch.

Old and expanded labels use the same pinned source bytes, captured Cargo/config,
SCIP files, CST files and frozen server/helper as the preceding raw-pointer
milestone. The adapter does not use rag's parser/resolver to choose oracle labels.
Original RA errors, ambiguous/generated exclusions, disabled-branch denominator
and build-configuration limitations remain. This is language-server source
correspondence, not independent rustc or runtime proof for every external call.

## Reproduction and integrity

For each pinned repository, run:

```sh
python3 scripts/qualify_rust_corpus.py SERVER CAPTURE SOURCE SCIP OUTPUT --external-descriptors
```

Then audit against the raw-pointer milestone's original results:

```sh
python3 scripts/summarize_rust_external_negatives.py docs/qualification/rust-raw-pointers-2026-10-26 OUTPUT_ROOT SUMMARY_JSON
```

The audit recomputes the oracle using the final adapter, rejects duplicate or
ambiguous source anchors, checks exact original per-case label/observation/result
preservation, input/config/SCIP and binary identity, metric recomputation and
native/actual-MCP parity. baseline artifact hashes are in summary.json. Script
hashes are in audit-script-sha256.json. Output files are create-new to avoid
silently replacing an earlier run.

Frozen server SHA-256:
172bde2c302ede19df0247c6b907d1710c9dc6013f1646a6e71cb23a35cbb91d.
Native helper SHA-256:
cca600b30661f9f0bdeece8e2d92d7ce3518d17a92219ff64dc895b9a074d41a.
Index format 68. All three native/MCP runs completed successfully. Five Python
adapter/metric tests passed and all four scripts byte-compile. Runtime semantics
were not edited; full Rust workspace tests were not repeated for this Python-only
change. The reused binary's preceding qualification is documented separately.

Source checkout is shared and not frozen; concurrent work is not attributed here.
Production binary unchanged and runtime qualification uses exclusively owned
disposable stores. All debug sync timings remain diagnostic; no release/RSS or
competitor advantage is claimed. Broader real repositories, third-party negatives,
remaining 111 original misses and complete compiler/build/performance requirements
remain open.
