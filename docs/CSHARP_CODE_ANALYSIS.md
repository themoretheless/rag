# C# code intelligence

Stored `.cs` sources now use tree-sitter-c-sharp 0.23.5. The pinned RAGRAF
`075caf86dc2f6274b9c48487e521915bcdfeaf2c` profile includes C# declarations,
namespace/using syntax, calls and base lists. This transfers that previously
missing syntax surface and adds captured namespace/owner/type candidates. No C#
compiler, build, application or source SQL is executed.

Declarations include file-scoped and block namespaces, nested classes, structs,
records (their source signatures retain class/struct spelling), interfaces,
enums, delegates, methods, constructors, finalizers, operators, local functions,
properties and individual fields. Exact source-bound byte ranges support explicit
body reads, including non-ASCII source prefixes. Signatures are bounded; field
initializers and receiver argument bodies are excluded from metadata. Generic
forms remain inventoried even when binding cannot be qualified.

Call references include invocation and explicit object construction. Nested calls
have separate name anchors, avoiding collisions when receiver expressions begin
at the same byte. Captured candidates use same-project namespace/type identity,
scoped namespace imports, literal using aliases, global usings, absolute global
names, implicit/this/base/static receivers, declared parameter/local types and
lexically scoped local functions. Namespace-relative imports precede outer type
lookup. Uniquely captured explicit constructors are linked; implicit constructors
are not fabricated. Equal-arity overloads preserve ambiguity; optional, generic,
named and ref/out/in applicability is unqualified. Argument types, conversions,
visibility and overload selection are not compiler-verified.

Declared base lists have separate `csharp_base_type_candidate` edges. Candidate
method lookup follows a single uniquely captured class base, bounded to 32 steps.
Interface/multiple-base/generic/external hierarchy lookup is incomplete; edges
never advertise a complete hierarchy. A project type inventory above 4,096
parsed documents fails type binding closed. Project membership is captured index
scope, not proven assembly or `.csproj` membership.

Parameters, local declarations, fields and properties (including captured inherited
members and unknown-base guards) prevent type-name guesses
through value shadows. Reassignment invalidates captured local/parameter typing.
Nested local functions retain enclosing value shadows. Unsupported tuple/pattern/
foreach/catch bindings make their routine lookup unverified. Lambda bodies,
dynamic receiver expressions, generic/primary-constructor owner and generic method contexts, explicit interface
implementations, duplicate/partial type declarations and conditional compilation
are not guessed. Conditional global imports also guard dependent lookups. Unknown
alias targets remain blockers rather than disappearing into global type lookup.

Static `Main`, public APIs and explicit constructor/finalizer lifecycle methods
are entrypoint candidates. Top-level statement entrypoints, method-group/delegate
binding, events, accessors as separate callables, synthesized record members,
implicit constructors, extension methods, using-static lookup, generated source,
source generators, preprocessor configurations, framework/test attributes,
assemblies/build graphs and runtime/virtual dispatch remain unqualified.
Structural slices/dead-code checks retain an explicit incomplete-analysis flag;
results are never deletion permission.

Extraction format 45 stores C# facts with serde defaults for older documents.
The store schema remains 25. Source synchronization, stale exclusion, exact body
reads, impact, binding-cache/full-recompute equivalence and frozen source/graph
history use the existing stores. `csharp_syntax_format=1` and
`csharp_relations_available` independently guard base-relation comparison against
legacy absent coverage. Cross-format symbol/call differences retain the existing
analysis-format warning. SQL literal extraction uses the existing collector; no
new SQL dialect or C# string-literal fidelity qualification is claimed here.

Official [namespace](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/namespaces)
and [expression](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/expressions)
specifications were inspected 2026-10-26 to inform guards; these fixtures do not
prove compiler conformance. Representative precision/recall, scale and complete
RAGRAF parity remain open.

Qualification: [csharp-index-2026-10-26](qualification/csharp-index-2026-10-26/README.md).
