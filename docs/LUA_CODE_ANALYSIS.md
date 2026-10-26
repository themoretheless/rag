# Lua syntax-index subset

`.lua` sources now use `tree-sitter-lua` **0.5.0**, pinned in Cargo.lock. The new
extractor covers named local/global functions, dotted/colon member declarations,
single-name function-expression assignments and anonymous function bodies.
Qualified names describe lexical/source ownership and table-name syntax; they
are not compiler-resolved module/type identities. Symbols and occurrences retain
captured document IDs, decoded UTF-8 byte spans, one-based source lines/byte
columns, revisions and content hashes. Existing `symbol`, `code`, references,
callers/callees, impact, repo_map, dead_code and structural slice consume these
facts. No Lua program, require target or source filesystem search is executed.

Lexical links resolve **bare local function candidates only**. Local function
statements are visible recursively. Local function-expression initialisers become
visible after the declaration, preserving Lua's distinction between
`local function f() f() end` and `local f = function() f() end`. Parameters,
local variables, nested blocks and numeric/generic for variables shadow outer
functions. Repeat-block declarations remain visible in the until condition.
Named table field keys are not bare reads; computed keys can contain value reads.

Named local-function value/callback reads are stored separately from calls.
Anonymous callback bodies are captured, but their invocation/liveness is unverified.
A local alias receives no
invented callable target, but its initializer can record a use of the original
function. Assignment to a local function binding, including from a nested closure
or a later non-local function statement, conservatively invalidates its links
throughout that binding's scope. Shadowed assignments do not invalidate unrelated
outer bindings. This is deliberately not execution-order/data-flow analysis:
conditional writes may make otherwise valid links unresolved.

Global names, table/member dispatch, metatables, `_ENV`, reflection/debug mutation,
require/package-path resolution, aliases, forward-declared function assignments,
multi-value assignment binding and runtime loading remain unverified. Table/global
function declarations are available as source symbols, but their calls are not
linked by a same-name guess. Implicit self in colon methods shadows an outer
local named self. Parenthesised/computed/returned call targets and richer dialect
semantics remain unqualified. Parse-error documents never gain static links.
Lua/LuaJIT bytecode/compiler/runtime validation is not claimed.

The extractor emits `LUA_TABLE_IMPORT_ENV_AND_RUNTIME_BINDING_UNVERIFIED`.
Language capability is partial_static with compiler/runtime verification false;
structural analysis remains marked incomplete even if all observed local targets
resolve. `resolved_static` means a lexical candidate, not proof of execution,
module liveness, runtime dispatch or safe deletion. Comments/long strings do not
create declarations or calls. Tree walking has a 256-level recursion guard.

The Lua extractor introduced syntax format **25**; later extraction extensions
may advance the current format (bulk SQL uses 26). Database schema remains 25. Older extraction
caches become stale until normal explicit index_code/source sync refreshes them.
Old immutable revisions keep their original facts and expose analysis-format
transitions in diff. No production cache/database was changed for qualification.
Only the Lua grammar and its root dependency were added to Cargo.lock; unrelated
transitive versions were preserved. SQL extraction from Lua strings, imports,
framework routes, representative precision/recall, larger/dialect corpora and
remaining reference languages are still open.
