# Rust receiver typing from captured AST facts

The format81 qualified runtime also retains selected imported standard arch
struct identities through generic wrappers and captured local trait bounds,
using explicit architecture/import/shadow guards. It does not create external
definitions or assume an external inherent-method inventory. See
[standard arch types](RUST_STDARCH_TYPES.md) and
[qualification](qualification/rust-stdarch-types-2026-10-26/README.md).

Format79 also propagates declared fn/Fn/FnMut/FnOnce callback outputs and standard
Option/Result success types through `?`, with lexical/standard-name/arity guards.
It infers the result receiver without inventing a concrete callback body. See
[callback outputs](RUST_CALLBACK_OUTPUTS.md) and
[qualification](qualification/rust-callback-try-2026-10-26/README.md).

The qualified format-78 runtime further propagates generic Self returns and
declared associated-type bounds through method/static-associated-call chains.
Targets remain trait declarations; cfg alternatives are selected independently
and unavailable/ambiguous/GAT/generic-trait cases retain conservative boundaries.
See [associated bounds](qualification/rust-associated-bounds-2026-10-26/README.md).

Format 76 additionally retains lexical generic parameter identity, trait bounds
and conservative implicit-Sized evidence. This permits pointer extension-trait
binding for `*const T`/`*mut T` with captured Sized parameters, while rejecting
`?Sized`, unknown and known-unsized pointees for Sized blanket impls. Distinct
parameters do not unify by matching trait sets. See
[generic-parameter qualification](qualification/rust-generic-parameters-2026-10-26/README.md)
for exact supported cases and remaining generic/compiler limits.

Index format **31** captures return annotations, async markers, named struct
fields, simple type aliases, lexical bindings/initializers and bounded expression
trees. `resolve::references` uses them for Rust member calls. Bare and qualified
references retain their existing binding path. Capturing syntax does not run a
compiler, proc macro, build script or source program.

Supported candidate inference:

- `service.user().save()` follows the selected method's declared return type;
- parentheses and `.await` preserve expression structure; only a known async
  function produces a future whose output can be unwrapped by await;
- `container.user.save()` follows a captured named field type;
- `[T; N]`, slices and standard `Vec<T>` indexing expose the element type;
- references and known standard `Box<T>`, `Arc<T>`, `Rc<T>` receiver wrappers
  expose their pointee; same-named local/imported custom wrappers are not std;
- `let user = factory(); user.save()` uses the factory's declared return;
- explicit function/method type arguments can substitute direct generic return
  placeholders, including generic arguments originating in another document;
- simple aliases follow the alias declaration context, with bounded cycle/depth
  handling. Return annotations resolve in the callee's document/module, not in
  the caller's similarly named types.

Each call occurrence is keyed by its start byte **and callee expression**.
Nested calls in a chain share their starting byte and must not overwrite each
other. Call edges preserve the existing reference identity, source revision/hash,
`provenance=derived` and explicit candidate set; their resolution basis is
`rust_ast_receiver_type`. `resolved_static` still means one supported static
candidate, not proven runtime dispatch or compiler validity.

Unknown initializers, shadowed/destructured values, reassignment beneath control
flow, unknown generic parameters, unawaited futures, custom wrappers and static
associated functions used as instance methods do not acquire guessed member
edges. Unresolved expression typing does not fall back to a same-name method.
Expression traversal, alias expansion, dereferencing and inference work have
separate depth/work limits. Exhausted inference produces no guessed target.

## Integration

Format 60 captures each method's impl-owner AST type as an optional Function
fact. Self lookup first respects an explicit substitution from a selected method
or impl match, then uses that captured owner rather than interpreting a display
name such as Worker<'a> as a nominal declaration name. Lifetime arguments remain
outside the nominal type-parameter slots; this is not lifetime/borrow checking.
Nested function items cannot inherit their enclosing method's Self context.

Declaration lookups without a qualified owner now start at the captured inline
module at the original byte position. Explicit self/super paths are normalized
from that declaration's inline module before Cargo lookup. This applies to impl
owner matching, returns and field types; an unrelated same-named root type is
not a substitute. Unresolved generic fields still cannot acquire a guessed
method. Generic Self with unproven type arguments, full associated-type/lifetime/
const-generic semantics and macro expansion remain under the existing limits.

Independent fixed-corpus and actual MCP/Cargo qualification:
[Self and declaration contexts](qualification/rust-self-context-2026-10-26/README.md).

`index_code` and source sync serialize the AST facts with each document's
extraction payload. The format bump refreshes older extractions. A changed return
annotation rebinds dependent calls when the graph is republished, even if the
caller's extraction is reused. Stale providers are excluded from current queries;
frozen historical graphs retain the original links. `index_status.capabilities`
documents the supported patterns and limitations.

## Remaining scope

This advances requirements 2 and 4 in `CODE_INTELLIGENCE_PLAN.md`. It is not a
complete Rust type checker. A bounded trait/impl extension now adds cross-file
owners, generic impl substitution, trait bounds and associated projections; see
[RUST_TRAIT_BINDING.md](RUST_TRAIT_BINDING.md). Complete impl/trait selection,
custom Deref/Index, operators,
closures as values, macros/cfg/features, re-export/visibility rules and full
compiler coercions remain open. Standard wrapper handling is intentionally
conservative around captured local trait implementations. General flow-sensitive
inference and live runtime targets are not claimed. Signature-only inference
assumes the captured source's declarations; syntax-only fixtures may contain
code a compiler would reject.

The checked-in positive runtime fixture independently passes compiler checking.
Its labelled calls are a small capability test, not representative precision,
recall, language completeness or comparative performance evidence. See
`qualification/rust-receivers-2026-10-25/README.md`.


Format 66 preserves cfg-selected named-field alternatives by exact struct owner
and source anchor. Unknown or duplicate alternatives cannot yield a unique type.
Direct std/alloc extern aliases can name supported collection/pointer wrappers;
reserved extern tags cannot collide with source identifiers or ordinary use
paths. Root/local shadows and captured standard-named dependencies block builtin
fallback. Nonstandard renamed extern aliases stay unverified. These are bounded
static rules, not compiler namespace or custom Deref/Index completeness. See
[RUST_BUILD_CONFIGURATIONS.md](RUST_BUILD_CONFIGURATIONS.md) and
[qualification evidence](qualification/rust-cfg-members-2026-10-26/README.md).


## Positional tuple struct fields (format 67)

Captured ordered field declarations now publish ordinal field names for tuple
structs. The existing nominal owner and generic substitution engine handles
nested projections such as `outer.0.0.value()` and `Pair<User>.0`. Visibility
modifiers are not counted as field slots. Out-of-range positions, enum variant
payloads and unknown field types do not guess method owners. Any attribute on a
tuple field currently suppresses extraction of that tuple layout, including
attributes that may be inert: cfg removal can renumber every following slot.
This restriction is explicit remaining work, not complete tuple/cfg semantics.
Built-in tuple values and constructor-pattern inference remain outside this rule.

The format bump invalidates older raw extraction caches. Independent fixed-label
corpus and actual MCP/compiler evidence:
[tuple field qualification](qualification/rust-tuple-fields-2026-10-26/README.md).


## Raw pointer extension-trait receivers (format 68)

Raw pointer types retain pointee type and const/mut identity separately from
references. Primitive AST types retain exact names for impl pattern matching.
Explicit cast target types participate in expression typing. Captured pointer
extension-trait implementations can supply method targets and existing method
substitutions carry Self through return chains. Pointer methods do not implicitly
dereference into pointee methods or interchange const and mut implementations.
Unknown pointees do not satisfy a generic impl. Known str/slice/dyn or unmodeled
pointees cannot satisfy an implicit Sized blanket parameter by shape alone;
complete Sized/unsized and ?Sized solving remains outside the captured model.

Raw-pointer inherent implementations are not fabricated from local source. Trait
implementations on pointer/primitive receivers require the implementation and its
trait declaration to share a captured target. Dot calls with known inherent
sysroot pointer method names remain unresolved rather than borrowing a same-name
extension-trait target. The reserved-name set follows the
[Rust pointer method inventory](https://doc.rust-lang.org/std/primitive.pointer.html)
checked on 2026-10-26 (Rust documentation 1.99.1); it is not runtime sysroot
introspection or a complete compiler method-precedence model. Primitive values
serve as exact pointer-pointee identities; arbitrary primitive dot-method lookup
is not added by this milestone. Pointer arithmetic return inference, coercions,
raw borrows, unsized metadata and dereference execution remain unqualified.

Evidence: [raw pointer qualification](qualification/rust-raw-pointers-2026-10-26/README.md).


## Declared union field receivers (format 70)

Union items now have captured nominal definition identities and generic parameter
facts. Named union fields use the same exact-owner/source-anchor/configuration
selection as named struct fields. Declared field types propagate through nested
members and generic substitutions; their methods are static candidate targets.
Union bodies are type scopes, and union definitions cannot act as callable value
constructors or hide a same-name function in the value namespace. Existing import
and namespace filtering applies to union types. Union definitions remain outside
the supported semantic-rename definition kinds.

This records the declared type of a field access. It does not prove the active
union member, initialization, unsafe-access validity, layout, aliasing or observed
runtime dispatch. Unknown/duplicate field alternatives do not yield a unique
receiver. Full union-related compiler semantics and flow-sensitive active-member
analysis remain unimplemented. Evidence:
[union field qualification](qualification/rust-union-fields-2026-10-26/README.md).
