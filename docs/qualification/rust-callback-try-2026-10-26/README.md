# Declared callback return types and standard ? receivers

The format79 AST model captures raw function-pointer and standard Fn/FnMut/
FnOnce declared outputs, including generic/where bounds, public imported aliases
and borrowed dyn callbacks. Invocation facts retain argument count without
breaking decoding of the old Call tuple. Standard Option/Result success types
propagate through `?`; local/custom/unverified standard namespaces cannot provide
that evidence. Generic parameters retain their identity and other trait bounds.

This resolves the result's method owner without claiming a callback body target.
Function-pointer type equality and generic impl matching are not inferred from
the partial callable shape. HRTB/variadic signatures, custom Try, argument-type
constraints and complete callable dispatch remain unsupported. Contract:
[RUST_CALLBACK_OUTPUTS.md](../../RUST_CALLBACK_OUTPUTS.md).

## Qualification

- The new callback/try native regression fails before implementation; its log is
  retained. Four new semantic regressions cover inline/generic/raw callbacks,
  Option/Result projection, arity/HRTB guards, aliases/where/borrowed callbacks,
  local callable/carrier shadows and an extern crate aliased as core. All
  **96 focused Rust tests pass**. Some negative snippets are deliberately
  compiler-invalid and qualify conservative AST behavior only.
- Actual stdio MCP verifies **five exact User/Other method targets** and ordinary
  warm revision reuse. Four callback invocations explicitly retain empty body
  target sets. Cargo executes **one passing test** checking five successful
  outputs plus None/Err early returns, using matching valid source.
- Fixed source/config/SCIP/CST and1183 labels with exact native/MCP candidate
  parity improve **473/570 ->478/570** positive correspondences. Memchr improves
  371/466 ->376/466; itoa and ryu are unchanged. Five recovered calls are the
  three generic iterator and two Rabin–Karp Pointer::distance occurrences.
  No correct site is lost and no false local target appears. All613 external
  negatives remain without local candidates;608 have observations and5 do not.
  Positive recall is83.86%; negative abstentions are not extra positive hits.
- Complete workspace: **1584 passed, zero failed, one ignored**. Strict library/
  test Clippy and five Python SCIP protocol/metric tests pass. Separate triage
  verifies all captured source hashes, oracle labels and per-case/report scores.

The frozen pair was copied before all corpus and actual-MCP runs: server SHA256
`5d7e288d1ad4eac4781b614ce7ab7cfcbe197fd1236414b090ab3d8a38beeecc`,
exporter SHA256
`fd8c2bebd490399a2c81b1b54bc2ffc5a2693773684a47c6109ac4f5e6788d0b`.
Runtime checks.json, exact scored/oracle/report cases, per-site transitions,
baseline hashes and complete logs are retained here. Build is development
profile. Shared dirty source contains concurrent unrelated work/tests; it is not
an atomic frozen source build and that work is not attributed to this milestone.
Current source hashes record audit inputs only. Production executable unchanged;
qualification uses exclusively owned disposable stores and compiler outputs.
No release performance or competitor advantage is qualified.

92 original positive misses remain:34 receiver,38 macro,18 missing observations
in disabled branches and2 other binding sites. Their original denominator is
preserved. Full compiler/Cargo/macros/callable semantics, representative quality,
equivalent-work release competitor measurements and all remaining original and
follow-up comparison milestones keep the complete plan active.

```sh
cargo test --offline --locked --lib rust_
cargo test --offline --locked --workspace
cargo clippy --offline --locked --lib --tests -- -D warnings
python3 scripts/smoke_rust_callback_try.py /path/to/frozen/rag-mcp /tmp/new-callback-runtime.json
```
