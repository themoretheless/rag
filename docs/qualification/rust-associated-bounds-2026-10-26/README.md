# Generic associated bounds and trait-return chains

The qualified format-78 runtime captures nongeneric trait associated declarations
such as `type Output: Done`, with exact owner identity, bounds, source position
and conditional availability. Generic receiver returns carry Self and their
selected trait's associated bounds through chains such as
`T::make().output().again().ready()`. Static associated calls preserve the same
receiver substitutions as member and explicit UFCS calls. Method targets remain
trait declarations; no concrete implementor or runtime vtable is invented.

Conditional associated declarations now participate in the existing cfg view.
Known-disabled alternatives are excluded, known-active declarations are used,
and unknown or duplicate alternatives cannot produce a guessed unique type.
Generic associated types and declarations on generic traits remain unsupported.
Optional/unknown bounds retain conservative Sized behavior. Bounds resolve in
the trait declaration's document and inline-module context.

## Verified behavior

- A new return-chain regression fails before the implementation, with an empty
  Factory::output target. The original failure is retained.
- Four new semantic regressions cover Self and associated-return chains,
  same-name outputs on distinct traits, missing/GAT evidence, optional Sized and
  unknown cfg. A cfg regression covers both feature alternatives and an unknown
  competing declaration. All **92 focused Rust tests pass**.
- Actual stdio MCP checks **eight exact trait declaration targets** across two
  feature configurations. Updating only Cargo.toml excludes stale targets,
  reuses unchanged Rust extraction, changes graph edges and frozen history, and
  ordinary warm indexing reuses the revision. Cargo executes **two passing
  tests**, returning 9 and 7 under the matching configurations.
- Unchanged source/config/SCIP/CST and all1183 labels: **467/570 ->473/570**
  correct positive targets. Memchr improves365/466 ->371/466; itoa and ryu are
  unchanged. Six recovered calls in generic/memchr.rs bind to the declaration
  CmpEq::movemask_will_have_non_zero in vector.rs. No correct site is lost and
  no false local target appears. All613 external negatives still have no local
  candidates;608 have observations and5 do not. Native/MCP sets match at every
  labelled site. Positive recall is82.98%, not the pooled exact-site rate.
- Final workspace: **1569 passed, zero failed, one ignored**. Strict library/test
  Clippy and five Python SCIP protocol/metric tests pass. A separate audit
  revalidates every source hash, oracle case, per-case score and report metric.

The first actual-MCP attempt exposed the unmodeled conditional-associated-type
barrier; the final cfg implementation and regression fix it. A preliminary full
workspace build also failed during a concurrent PHP context-ID interface edit.
That shared interface was subsequently corrected by the other ongoing work;
the final full rerun passes. Both failures are retained under preliminary/ and
are not accepted evidence. Concurrent unrelated PHP changes are not attributed
to this feature. Initial code used format77; the final shared format78 includes
the concurrent schema update and is the only accepted runtime pair here.

Frozen final server SHA256:
`17ffb90de46f54298382f9cbba95a91b3bc9be3709a1af1628cc8e45d1e01b30`.
Native exporter SHA256:
`b3e36f5931b41a594cda6cf6ecd01f7a632b6e165dc5193fd642a210af6e30da`.
The pair was copied before all final corpus and MCP runs. Build is development
profile; current source hashes do not constitute an atomic frozen build snapshot.
This does not replace the separate release performance candidate or establish
speed/memory/competitor superiority. Qualification stores and compiler outputs
are disposable and exclusively owned; production executable is unchanged.

## Remaining scope and reproduction

97 original positive labels remain misses:39 receiver,38 macro,18 missing
observations in disabled branches and2 other binding sites. The original
denominator is unchanged. Full associated equality constraints, GAT/HRTB,
supertrait/generic-trait inference, compiler/macro/Cargo semantics and broader
real-world quality remain open. Source-definition correspondence is not
universal/compiler/runtime truth. The original plan and its follow-up comparison
milestones remain active.

```sh
cargo test --offline --locked --lib rust_
cargo test --offline --locked --workspace
cargo clippy --offline --locked --lib --tests -- -D warnings
python3 scripts/smoke_rust_associated_bounds.py /path/to/frozen/rag-mcp /tmp/new-associated-runtime.json
```
