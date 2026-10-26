# PowerShell code intelligence

Stored .ps1, .psm1 and .psd1 sources now use tree-sitter-pwsh 0.38.1.
The pinned RAGRAF 075caf86dc2f6274b9c48487e521915bcdfeaf2c profile includes
functions, classes, enums, command/member calls and using directives. The cached
tree-sitter-powershell 0.26.4 lacks using-statement nodes; it was inspected but
not introduced. The selected maintained grammar supports using directives and
uses the corrected invocation_expression node spelling.

The actual MCP corpus exposed an upstream grammar error on pure double-quoted
using-module paths. Format 48 retries parsing with only those header quotes
normalized to single quotes. Only constant literals without interpolation,
escapes, embedded quotes or unexpected trailing code qualify. The adapter stays
within top-of-file directives/comments, preserves every byte offset/line and
continues to read all source evidence and bodies from the original text. It
emits POWERSHELL_LITERAL_USING_QUOTES_NORMALIZED. Other parser errors remain.

No PowerShell interpreter, runspace, module load, script or manifest is executed.
Parser availability and these fixtures are not PowerShell conformance.

Definitions include function/filter/workflow syntax, classes, methods/explicit
constructors, properties and enums. Symbols retain original name spelling and
source byte ranges. Method qualified owners and call names use canonical ASCII
case folding; broader Unicode/.NET culture behavior is unqualified. Signatures
are bounded and stop at the declaration's opening brace. Property initializers
and comments inside function/method bodies are excluded from metadata; exact
body reads remain explicit.

Unqualified commands and literal ampersand command names can link to captured
same-file functions, selecting the nearest captured syntactic scope. Top-level
literal dot-source imports can add captured root functions. This is not proof
of dynamic caller scope, definition execution, ambient aliases/cmdlets, command
precedence or availability before declaration. Unknown/scoped function names,
conditional declarations, deferred script-block bodies, aliases, unknown loaders,
eval/provider mutations and captured export changes retain unverified states.
Computed command/script-block targets keep inner occurrences without assigning
a fictitious executed scope. Same-name local/imported root functions preserve
ambiguity rather than guessing dot-source load order. Function-local dot sourcing
is inventoried but makes binding unverified instead of leaking its definitions
into other scopes.

Literal top-level dot sourcing through "$PSScriptRoot/relative.ps1" or the
backslash spelling resolves captured source-relative path candidates. Bare
"./lib.ps1" dot sourcing remains unresolved because the process working directory
is not established. Literal source-relative using-module paths add separately
captured class/type scope; they do not invent function exports. Names, assembly/
namespace directives, hashtable module specs, wildcards, interpolation and search
paths are not reconstructed. Paths use exact stored spelling and same-project
fresh documents. No Windows case-insensitive filesystem identity is guessed.
Unavailable, duplicate or stale load targets prevent successful scope binding.
Closure traversal is bounded to 4,096 source documents.

Member calls on literal [Type]::Method and instance $this.Method can link to
captured methods. Static/instance identity and positional argument counts filter
candidates; equal-arity overloads remain ambiguous. Explicit [Type]::new links
only to a captured matching constructor; implicit constructors are not invented.
A uniquely captured single class base can provide inherited candidates, bounded
to 32 steps. Unknown object types, conversions, return/field chains, interfaces/
multiple bases, generic/external types, base initializer execution and virtual
runtime dispatch remain unqualified. Base edges never claim a complete hierarchy.

Captured module functions, explicit constructors and conventional Main/Invoke-Main
are entrypoint candidates. Actual exports, script entry execution, Pester/DSC,
remoting, profiles, jobs, workflow execution and deployment remain unqualified.
Structural slices/dead-code checks retain incomplete-analysis diagnostics;
candidates are never deletion permission.

Extraction format 47 initially stores PowerShell facts with serde defaults for legacy
documents; format 48 adds the literal using-module quote adapter. Store schema 25 is unchanged. Source sync, stale exclusion, exact
body navigation, impact, cache/full-resolution equivalence and frozen history
use existing stores. powershell_syntax_format=1 and
powershell_relations_available independently guard base/file-load relation
comparison against missing legacy coverage. Cross-extractor symbol/call changes
retain the existing format warning.

Official [command precedence](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_command_precedence)
and [scope](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_scopes)
documentation and the actual grammar/AST were inspected 2026-10-26. Representative
semantic precision/recall, performance/scale, runtime and full RAGRAF parity
remain open.

Qualification: [powershell-index-2026-10-26](qualification/powershell-index-2026-10-26/README.md).
