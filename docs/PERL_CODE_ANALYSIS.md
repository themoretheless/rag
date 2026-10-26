# Perl code intelligence

Stored `.pl`, `.pm` and `.t` sources now use `ts-parser-perl 2.0.0`, the maintained
`tree-sitter-perl/tree-sitter-perl` grammar family used by the reference language
pack. The similarly named cached `tree-sitter-perl 1.1.2` belongs to a different
repository and AST; it was inspected but not added. This avoids mapping a
reference profile onto incompatible node names. The pinned reference is RAGRAF
075caf86dc2f6274b9c48487e521915bcdfeaf2c, including its Perl variable extension.
No Perl interpreter, source program, module loader or source SQL is executed.

Declarations include semicolon/block packages, subroutines, forward declarations,
lexical subs, anonymous subs, modern class/method syntax, phasers and scalar/
array/hash lexical and `our` variable declarations. Signatures are bounded;
variable metadata retains the declaration spelling without its initializer.
Source spans support explicit exact body navigation. Semicolon packages affect
subsequent statements in their lexical block; leaving a block restores the
surrounding package. Modern class methods are inventoried without inventing an
instance type or treating them as ordinary package sub calls.

Explicit function and literal package-method calls have source-bound occurrences.
Captured same-project package/sub names, lexically scoped subs and a uniquely
captured single `use parent`/`use base` ancestry can produce static candidates.
Qualified function calls do not inherit through `@ISA`. Duplicate definitions
preserve ambiguity; bodyless forward declarations do not become implementations.
Unknown object receivers, coderef calls, dynamic names and ambiguous bareword/
spaced-call grammar forms remain unbound. Function value occurrences are separate
from calls. Nested receiver argument bodies are excluded from metadata. A call
and variable read sharing one byte offset retain distinct facts, preventing a
variable ID from becoming an invented callee.

Variable references distinguish sigils and lexical scopes. Initializer reads
precede visibility of the new declaration, so an outer `$x` is not replaced by
`my $x = $x`. Signature parameters and captured anonymous-sub parameters have
source declarations. Direct `$h{key}`/`$a[index]` container syntax normalizes to
`%h`/`@a`; scalar-reference dereference syntax keeps `$ref`. Package declarations
and variable aliases remain candidates, not proven symbol-table identity.

`use`/`require` and supported literal parent declarations preserve file import
candidates using exact captured same-project package declarations, independently
of the module filename. They do not resolve the actual `@INC`, load order,
Exporter rules, installed modules or CPAN. Unknown import/export effects, eval,
localization and captured symbol-table mutations guard affected package binding.
Literal `use Module ()` does not invent exported names. Supported syntax pragmas
are not treated as arbitrary exporters; their runtime identity remains unproven.
Parent lists, conditional/dynamic loaders, `mro`, class attributes and all module
side effects are not fully reconstructed. Single-parent lookup has a 32-step
bound; package lookup above 4,096 parsed project documents fails closed.

Phasers, conventional `main::main`, `DESTROY` and routines in `.t` files are
entrypoint candidates. Framework identity, top-level execution/liveness,
Moose/Moo roles, dynamic dispatch, full import/prototype semantics, arbitrary
coderef flow, lexical experimental forms and compiler/runtime conformance remain
unqualified. Structural slices/dead-code checks keep incomplete analysis explicit;
candidates are never deletion permission.

Extraction format 46 stores Perl facts with serde defaults for older documents;
store schema 25 is unchanged. Body reads, source sync, stale exclusion, impact,
cache/full-recompute equivalence and immutable history use existing stores.
`perl_syntax_format=1` and `perl_relations_available` independently guard parent
and package-file relation comparisons against absent legacy coverage. Cross-
extractor symbol/call changes still carry the existing format warning. The SQL
collector captures tested ordinary static literals with enclosing sub ownership;
broader Perl quoting/interpolation and SQL dialect support remain unqualified.

Official [subroutine](https://perldoc.perl.org/perlsub) and
[package/module](https://perldoc.perl.org/perlmod) documentation and the actual
parser AST/grammar were inspected 2026-10-26. This qualification does not prove
representative precision/recall, scale or full RAGRAF parity.

Qualification: [perl-index-2026-10-26](qualification/perl-index-2026-10-26/README.md).
