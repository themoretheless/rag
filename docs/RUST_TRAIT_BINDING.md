# Captured Rust trait and impl binding

Index format **32** adds explicit impl ownership, generic parameter/bound facts,
trait method declarations and associated type assignments to the AST receiver
engine. No source program, compiler, build script or proc macro runs in indexing
or queries.

Implemented static candidate rules:

- An impl in another captured file is matched to the actual owner declaration,
  not to another same-named struct. Known Cargo targets constrain impl ownership;
  a binary/test impl is not silently an inherent method of the library's type.
- Generic struct arguments bind generic impl parameters. Matched substitutions
  flow into method returns and named fields; known local trait bounds on an impl
  must have captured implementation evidence.
- Inherent methods take precedence over visible trait candidates. Imported trait
  aliases are considered. Multiple remaining trait methods stay ambiguous.
- `<Type as Trait>::method` selects the specified trait's matching impl/default.
- `T: Trait`, `where T: Trait`, `dyn Trait` and parameter-position `impl Trait`
  expose **trait declarations**, not guessed concrete runtime implementors.
- Matched impls substitute `Self` and `Self::Item`; explicit generic function
  arguments can resolve `T::Item` through captured bounds. Qualified projections
  such as `<Type as Trait>::Item` use the selected associated assignment.
- Trait default bodies remain candidates when the matched impl does not override
  the method; their return types can use the matched Self/associated assignments.

Every edge remains `provenance=derived`. A link to a trait declaration uses
`resolution_basis=rust_trait_declaration`; other typed links retain
`rust_ast_receiver_type`. A unique declaration can be statically bound without
proving a concrete runtime vtable target. `index_status.capabilities` exposes this
distinction and the remaining limitations.

Captured impls with unknown conditional presence or negative-impl syntax are not
used as positive evidence. Resolution has depth and work limits and never turns
exhaustion into a guessed target. Project identity and source revision/freshness
remain part of the surrounding graph contract.

## Remaining boundaries

This is a bounded source-semantic subset, not a complete Rust trait solver.
Coherence/specialization, all orphan-rule details, higher-ranked bounds, generic
associated types, full supertrait traversal, trait generic-argument inference,
associated equality constraints, complete extern-prelude/re-export/privacy rules,
compiler coercions, macro expansion and feature/cfg/target configurations remain
open. Shared-file multi-target symbol instantiation is not implemented. Unknown
stdlib/external trait implementations cannot satisfy a bound by assumption.

Source without captured Cargo manifests retains the earlier layout fallback;
this is not proof of crate membership. Positive fixtures are independently
compiler checked; some negative unit fixtures intentionally have only syntactic
validity. Fixture qualification does not establish representative Rust graph
precision/recall or comparative speed.

See `qualification/rust-traits-2026-10-25/README.md` and the complete
`CODE_INTELLIGENCE_PLAN.md` for acceptance status.
