# Frozen Rust quality miss inventory

The same-release corpus still measures 465/570 positive target correspondences,
zero false local targets and 613 external-negative labels. This audit preserves
all labels and scores; it adds no resolved targets and makes no speed claim.

`scripts/triage_rust_quality.py` rechecks every captured Cargo/source input hash,
oracle-case hash and exact case order/content, native/MCP parity count, unique
site/symbol identities, all per-case TP/FP/FN/exact scores and aggregate metrics.
It then inventories missed sites with original byte-column source/definition
anchors and surrounding source. Output creation refuses an existing file.

| Observed category | Sites |
|---|---:|
| Receiver observation with no target | 47 |
| Unverified macro expansion | 38 |
| No extracted observation | 18 |
| Other unresolved name binding | 2 |
| Total | 105 |

These categories describe evidence, not established root causes. Full raw cases,
targets, observation bases, excerpts and original report digests are in
`triage.json`; aggregate audit receipts and script hashes are in `checks.json`.
Four regression tests pass, including forged scores/labels, source drift,
UTF-8 byte columns, traversal and symlink escape guards. No Rust source changed;
the Rust workspace suite was not repeated.

## Configuration boundaries found by source inspection

All 18 missing observations occur inside disabled source branches in the captured
library configuration. This is a source/configuration inspection, not an
independent compiler proof for every labelled call:

- ryu `src/f2s_intrinsics.rs:84,105`: two calls inside `cfg(feature="small")`;
  captured Cargo graph has no enabled small feature (ryu declares no default one).
- memchr `src/memmem/searcher.rs:80,91,115`: three calls in x86_64/sse2 or
  wasm32/simd128 blocks; captured target is aarch64-apple-darwin.
- memchr `src/memchr.rs:761,769,777,785,792,802,813,824,835,847,859,871,883`:
  thirteen calls in the `cfg(test)` module beginning at line 753; captured target
  kind is lib and cfg flags do not include test.

These 18 remain misses in the original 570-positive denominator. Publishing a
different, configuration-aware metric would require an independently labelled
oracle version and separate results. Reintroducing disabled edges into the
active graph merely to increase recall would be incorrect.

## Next implementation work

1. Preserve identity and declared bounds of generic parameters through receiver
   inference. Current unbounded parameters become Unknown; pointer blanket impl
   matching deliberately rejects unknown pointees. Any extension must distinguish
   implicit Sized, explicit ?Sized, trait-object/slice/str and shadowed parameters.
   The 47 receiver sites also include field chains and trait-method return chains;
   they are not all proved to share the same cause.
2. Add optional compiler-derived macro expansion evidence rather than treating
   arbitrary tokens inside macros as executable calls. Keep source/expansion
   mappings, configuration identity and unsupported-state reporting.
3. Reproduce the two module/shadow binding cases with exact negative expectations.
4. Retain the fixed corpus and independently add feature/test/target-specific
   labels. Run each under its own captured configuration; never merge their
   active edges into one configuration's graph.

Reproduction:

```sh
python3 -m unittest discover -s scripts -p test_triage_rust_quality.py
python3 scripts/triage_rust_quality.py \
  docs/qualification/same-release-rust-quality-2026-10-26 \
  /tmp/rag-real-corpus/pinned /tmp/new-rust-triage.json
```

The original ten-requirement plan remains open. This milestone supplies an
audited backlog and configuration evidence, not a compiler-complete resolver,
competitor comparison or completed product feature.
