# Declared Rust callback outputs and standard try carriers

Format79 captures function-pointer and parenthesized Fn/FnMut/FnOnce signatures'
argument count and declared output type. Inline impl/dyn traits and generic/where
bounds can carry this return evidence; borrowed known callable values retain it.
Imported public core/std Fn-trait aliases are normalized and checked against
captured local/generic/extern/dependency shadows. No callback body is executed or
selected by this return inference.

New invocation expressions retain raw argument count; the old serialized Call
variant remains decodable for stale-cache migration. A callback invocation with
matching arity and no unsupported explicit type arguments can supply its declared
result type. Generic parameter identity/trait bounds remain distinct from the
optional callback output. Other declared local trait-bound methods retain their
declaration targets. Function-pointer shapes are not newly admitted as generic
impl patterns merely because their return types and arities match.

The try expression `?` projects the success type only from a recognized standard
Option<T> or Result<T,E>. Canonical core/std option/result paths and imported
aliases are supported; a captured local same-name nominal type is not standard.
Unverified or shadowed core/std namespaces cannot supply standard evidence.
Type aliases use their existing captured declaration context. This is static
success-type projection: runtime failure, FromResidual/error conversions, borrow
checking and the caller's residual compatibility are not inferred.

Examples include `let p = find_raw(start,end)?; p.distance(start)` where the
callback declares Option<*const u8>, and `factory()?.save()` where a captured
function returns Result<User,E>. The callback invocation itself may remain
unresolved while the receiver after it has a declared type. It is incorrect to
invent a concrete callback body from the method name or return type.

Unsupported cases fail closed: unknown/untyped callback values, custom Try,
higher-ranked/variadic signatures, unverified callable-trait names and ambiguous
callable bounds. Matching-arity argument types are not checked; the declared
output assumes a valid invocation. Callable fields/closure-body inference and full invocation dispatch
remain outside this subset. Reassignment invalidation, scope/freshness guards and
existing work/depth budgets still apply. Arity does not establish compiler validity.

Evidence: [qualification](qualification/rust-callback-try-2026-10-26/README.md).
No compiler, callback, build script or source program runs during index lookup.
