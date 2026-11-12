# Captured standard architecture type identity

The qualified format81 runtime retains identity for a bounded catalog of public
standard architecture structs imported through core::arch/std::arch. This lets
generic wrapper arguments, tuple/named fields and captured local trait impls
participate in receiver inference instead of collapsing the argument to Unknown.
No external definition node or intrinsic body is fabricated.

Identity requires a unique captured target_arch, an exact catalog path, trusted
standard namespace and valid source binding. std paths normalize to the same
core identity. Explicit imports, type/module aliases and a single visible glob
are supported. Unknown or competing globs, local/generic shadows, unverified
externs/dependencies and mismatched/unknown architectures fail closed. Bare type
spellings without import evidence do not acquire a standard identity.

Rust glob imports are now retained as bindings, including their cfg availability.
The existing general trait-import enumeration excludes these new glob records;
this does not silently enable arbitrary wildcard name resolution. A disabled
optional dependency named core/std/alloc is distinguishable from an active one
only in a successfully selected configuration. Raw/unavailable configurations
retain the conservative dependency barrier. Active dependency and root-alias
shadows still block standard identity.

Catalog identity can establish a captured local trait implementation for generic
bounds. Exact owner/type/trait identities, target membership and orphan-style
local-trait guards still apply. The trait solver cheaply rejects clearly
different trait spellings/import aliases before expensive owner matching, but
every surviving candidate still needs its exact declaration identity and bounds.
Work limits are unchanged; exhaustion still clears partial results.

Direct method/associated-call lookup on an external architecture value remains
unverified because its inherent-method inventory is absent. An explicit UFCS
trait selection can select a captured local impl without that inherent ambiguity.
The catalog does not establish ISA support, stability/feature availability,
compiler validity, ABI/layout or execution safety.

## Catalog provenance

`generate_stdarch_catalog.py` captures selected public struct names and source
positions from installed rust-src, plus source hashes for their declaration/
macro/export files. Its output is compiled as a static catalog; graph queries do
not read the toolchain or run Cargo/rustc. Current selected module entries:
aarch64 112, arm104, x8616, x86_6416, wasm321. These counts include related arch
types and are not a language-support or compiler-validation inventory. Other
architectures/items remain unknown. Catalog updates require source/provenance
review, cache-format invalidation and qualification.

Actual compiler/runtime qualification covers the aarch64 uint8x16_t/uint16x8_t
fixture, not all catalog entries or targets. Source hashes, deterministic
regeneration and declaration positions are retained in
[qualification](qualification/rust-stdarch-types-2026-10-26/README.md).
