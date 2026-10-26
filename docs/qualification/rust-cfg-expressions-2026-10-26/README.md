# Conditional Rust expression qualification

Formats 53/54 project cfg on call arguments, statement expressions and blocks
without blocking every binding in the source file. This advances requirements
1/2/4 of the original plan; the complete goal remains open.

| Pinned library | Format 52 correct / labelled | Format 54 correct / labelled |
|---|---:|---:|
| itoa | 9 / 9 | 9 / 9 |
| ryu | 56 / 95 | 91 / 95 |
| memchr | 107 / 466 | 164 / 466 |
| Combined | 172 / 570 | 264 / 570 |

The unchanged SCIP definition-correspondence denominator yields 30.18% to 46.32%
recall. Direct-call sites improve 156/486 to 239/486 (32.10% to 49.18%); imports
improve 16/39 to 25/39. Macro tokens and value/other sites remain separate.
Observed false targets remain zero in both versions. These numbers are not
universal Rust precision/recall: the corpus is small, excludes ambiguous/generated
anchors and has no admitted external-negative sites. Some SCIP references are
present even in cfg-disabled code, as the separate compiler audit below shows.

`summary.json` verifies matching input, SCIP and label hashes plus complete
native/MCP candidate parity. Baseline reports/labels/scored cases remain under
`../rust-real-corpus-2026-10-26/controlled-{itoa,ryu,memchr}-after/`; their exact
hashes are in `baseline-artifacts.json`. New raw reports are in the three local
`controlled-*-after/` directories. Corpus commits, original source bytes, explicit
compiler/default-library configurations and oracle provenance are unchanged
from the baseline. No labels were removed to obtain this before/after result.

## Semantics and safety

References and semantic facts inside inactive/unknown expressions are excluded;
unknown conditional mutations retain their invalidations so a later receiver
cannot inherit a potentially obsolete initializer type. Captured field types now
have source positions and undergo the same filtering. Conditional let bindings,
parameters, fields and other unsupported syntax still retain conservative
limitations where their projection is unmodeled. No inference from argument
values/arity or inferred block return values is claimed.

Permission decisions capture enclosing return/control-flow syntax rather than
independently projected argument/body facts. Such decisions are suppressed when
an argument, statement or block condition is false or unknown. Unknown syntax
does not become an implicit permission allow. The preliminary test/not(test)
overlay now preserves feature/platform import facts until the actual captured
build configuration is applied, avoiding irreversible loss of feature imports.

Native regressions cover both feature choices for conditional argument and body
calls, wrong/inactive target exclusion, unaffected calls, unknown versus disabled
mutation, policy suppression and feature imports in the test overlay.
`runtime.json` uses actual stdio MCP and independently labelled compiler-valid
sources in `tests/fixtures/code-intelligence/rust-cfg-arguments`. Both feature
selections produce exactly the expected argument/body call targets and discover
config_test through affected_tests. Each suggested Cargo command is actually run
against that source/configuration and executes one passing test.

## Independent compiler anchor audit

`compiler-anchor-audit.json` records another independent source-activity check for
ryu using the same recorded rustc and target/default features. Cargo produces
nightly `-Zunpretty=hir-tree` output; input hashes are checked against the scored
run before and after. The raw HIR digest and exact compiler command are retained;
the disposable 9 MiB HIR file stays under /tmp rather than being vendored here.

Only exact ASCII lexical path spans (allowing a qualified path prefix) are
admitted, with no dependency on the rag AST. This corroborates 68 of 71 SCIP
direct-call source anchors; rag finds 66/68 with zero observed false targets.
Two missing anchors are the small-feature-disabled calls in
src/f2s_intrinsics.rs:84 and :105. A third unverified anchor is a valid method
call whose HIR representation is outside this deliberately strict path adapter.
Therefore absence of a matching HIR anchor never proves inactivity or invalidity.
Target definitions still come from SCIP; this does not validate rustc target
resolution or runtime dispatch. The original 570-site comparison stays intact.

The remaining corroborated ryu misses are raw::format32/raw::format64 re-export
chains. Memchr still has receiver typing, nested cfg and macro-expansion gaps.
General compiler visibility/re-export semantics, broader independent corpora,
external negatives and equal-work release/competitor speed/RSS remain open.

## Reproduction and checks

Use the pinned preparation described in the baseline README. Run
`qualify_rust_corpus.py` for each repository with a frozen format-54 server/helper
pair, and `summarize_rust_quality.py` with baseline directories named
`controlled-*-before` and new directories named `controlled-*-after`.

```sh
python3 scripts/smoke_rust_cfg_arguments.py /path/to/frozen/rag-mcp /tmp/new-runtime.json
python3 scripts/audit_rust_compiler_anchors.py /tmp/corpus/ryu /tmp/results/controlled-ryu-after /tmp/new-compiler-audit
cargo test --lib code_index::rust_
cargo clippy --lib --tests --examples -- -D warnings
```

The final focused Rust suite passes 45 tests. The full workspace passes 1445
tests, with zero failures and one ignored external-DB fixture. Strict Clippy,
three Python adapter/metric tests and both actual MCP feature configurations
pass. Full results are retained in checks.json/workspace.log. The server SHA-256 is
1f8de5815a3fc3a08c6f54fe318f9da41e781a71a74c64e4b71e9b6d72e259ae;
the native helper SHA-256 is
bc556080036bb5b74a2655a99fe6197f6b3f0cf1636d7f18c8c12fef5010516a.
All corpus and fixture runtime results use that server. The shared dirty checkout
contains concurrent unrelated work and was not frozen. Qualification databases
are disposable and opened exclusively in sequence; production DB/installed MCP
are untouched. Timings are uncontrolled debug diagnostics, not speed evidence.
