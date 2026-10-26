# Captured Cargo-local Rust re-exports

Formats 56/59 add a separate captured namespace inventory: module-scoped use
declarations, aliases, grouped imports, module globs and AST item visibility.
Normal and test-cfg views filter the inventory using captured configuration;
unknown imports retain an explicit unverified state instead of disappearing and
leaving a misleading unique target. Raw extraction remains reusable.

Known Cargo module paths can follow explicit crate/self/super re-export chains
to original definitions. A re-export inside a path dependency resolves against
that dependency's captured module map, so a consumer can call a public facade
without seeing a private implementation module. Function/type identities stay
those of their original source definitions. Same-file inline module imports are
now lexically scoped to their module instead of leaking into the containing file.

Explicit named items take precedence over module globs. Distinct candidates from
multiple globs remain ambiguous; duplicate named imports and unknown alternative
imports do not produce partial unique bindings. Cyclic module globs can reach
known definitions without infinite recursion. Namespace lookup is bounded to
512 recursive visits and depth 16; exhaustion clears all candidates already
found in that lookup. Capture is bounded to 200000 visited AST nodes, 2048 import
entries, 64 module levels and 32 nested use-list levels. Unsupported/incomplete
capture is not treated as proof of absent competing exports.

New re-export paths check pub, pub(crate), pub(super), pub(self), private and
pub(in crate/self/super paths) regions against the requesting captured crate and
scope. Re-exports cannot widen a private target's visibility. Named module/type
aliases check the base before exposing children; forwarded child paths cannot
use the exporting module's private access to expose a private nested module.
Function-local declarations and associated methods are not importable free
namespace items. Public methods remain usable through valid exported type
aliases. A Type::* expression is not assumed to be a module glob.

These are conservative static candidates, not full rustc namespace/privacy
verification. Existing direct bindings retain their previous scope. Full separate
type/value/macro namespaces, extern-prelude/leading-:: paths, arbitrary dependency
re-export hops, unqualified alias chains, enum globs and compiler macro expansion
remain unmodeled. Inline caller scope is conservatively approximated by its
captured external module for restricted access, and multi-unit Cargo identity
retains the limitations in RUST_BUILD_CONFIGURATIONS.md. Some valid restricted
paths can therefore remain unresolved. Do not infer compiler acceptance from a
candidate graph edge.

Existing code query surfaces return original source occurrences and target IDs;
they do not yet expose a separate per-hop re-export evidence list. Unsupported
namespace lookup may report a general unresolved module/import basis rather than
an individual hop diagnostic. Representative namespace coverage and diagnostic
qualification remain open.

Qualification: [native, MCP, compiler and fixed-corpus evidence](qualification/rust-reexports-2026-10-26/README.md).
