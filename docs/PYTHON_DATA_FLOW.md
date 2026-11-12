# Python value-dependency slices

`data_slice` is a source-bound value-dependency query, separate from the existing
structural call `slice`. The native backend captures plain Python module-level
functions with sequential simple assignments, local reads, scalar literals,
arithmetic/comparison expressions, direct positional calls, `if/elif/else`, `for/while` loops, explicit completions through `try/except/else/finally` and explicit returns.
It records actual syntax occurrences and source identities rather than using
function-call reachability as a substitute for value flow.

```json
{"symbol":"left","project":"alpha","direction":"back","depth":12,"limit":200,"max_work":50000}
```

The function selector accepts an exact symbol ID, qualified name or short name;
project/path filters disambiguate it. Backward traversal seeds explicit returns,
forward traversal seeds parameters, and both seeds their union. `variable`
selects a local definition or parameter; multiple writes require the exact token
`start_byte`. Byte ranges refer to UTF-8 decoded stored source, not original
on-disk bytes. `code` can read the enclosing actual function via `node.function`.
Responses contain occurrence metadata, source hashes/revisions and no full bodies.

Local and captured shadow/rebinding keys use Python NFKC identity while occurrence names and
byte spans preserve their original spelling. Compatibility spellings can
therefore overwrite the same local binding; variable selectors also match that
normalized identity. Local bindings retain the latest preceding definition. A later overwrite does
not make an earlier definition an input to subsequent reads. Edges distinguish
`def_use`, `expression_input`, `assignment` and `return_value`. Captured uniquely
bound functions with matching positional arity add `call_argument` and
`return_to_call` edges. No generic argument-to-call-result shortcut is added:
a literal constant return can therefore be independent of an unused parameter.
Known module rebinding and unqualified source/header/module effects block those
interprocedural boundaries. Imports use the existing project-scoped static
resolver; unknown/ambiguous/unqualified boundaries remain explicit diagnostics.

Branches produce `merge_input` edges from distinct reaching definitions on paths
that can continue. A path with an explicit return cannot supply a later local;
a missing assignment on a continuing path makes a later read unsupported.
An absent `else` retains the incoming local values as another possible path.
Synthetic merge nodes have source ranges and no fabricated source token names;
variable selectors continue to select actual definitions/parameters only.
`control_dependency` edges record conditions governing captured occurrences and
merges. Unrelated conditionals do not influence a subsequent constant return.
`branch_control_dependencies=true` describes this bounded model. It does not
prove predicate polarity, feasibility or correlation between separate branches;
`branch_path_feasibility_proven=false` and `control_flow_modeled=false` remain
explicit. Traversal witnesses may combine syntactic candidates that cannot occur
on one execution path. There is no complete CFG, exception or loop analysis.

Loop headers retain incoming values and `loop_backedge` inputs from normal body
completion and captured `continue` states. `break` states feed separate loop-exit
merges and skip `else`; return paths never feed the next iteration. Nested jumps
target the innermost active loop body. Normal loop exit runs `else`, including
the zero-iteration path. Loops always retain a possible zero-iteration path:
constant predicates and iterator cardinality are not proven. Body-only names
cannot initialize a later read unless all continuing exit paths define them.
`for` supports a plain identifier target and an abstract `iteration_item` with
`iteration_value` provenance from the iterable expression, evaluated once before
header merging. It does not identify individual elements, types, cardinality,
iterator protocol effects or heap aliases. Async iteration and destructuring
remain unsupported. `loop_carried_dependencies=true` does not prove iteration
feasibility, termination or concrete runtime results; both corresponding proof
flags remain false. The model computes cyclic may-dependencies, not unrolled
iteration counts or a complete CFG.

Finalizer completion routing and conservative protected expression-error prefixes
are described in [the finalizer contract](PYTHON_FINALLY_FLOW.md).

Every call site, including its captured finalizer completion context, has a frame identity. Traversal pushes/pops matching frames when
crossing argument/return boundaries, so values from one call do not escape
through another call's result. A function selected as a root has an unbound
outer invocation and can include its captured callers. Per-occurrence witnesses
retain the exact stack transitions, physical edge direction and source evidence.
`edges` includes checked reachable transitions, including cycles and alternative
edges that a single predecessor witness would omit. Each physical edge carries
one matched `transition` context (direction, from/to stacks and reachable-source
assertion); this context can differ from a node's chosen representative witness.
Additional contexts for the same edge are not enumerated. The edge cap has an
explicit `edge_limit_reached` flag and prevents a complete-subset claim when hit.
The returned edge union is not asserted to be one coherent path; each node
witness is a separately matched traversal from a selected seed. Runtime paths remain
unproven.

Capture uses format 7 in extraction/index format 87; earlier value-fact formats 1/2/3/4/5/6
and missing capability payloads require repair. The existing syntax-cache,
source-sync and one-read-transaction freshness flow applies; missing capability
facts trigger cache repair. Immutable revisions freeze captured value facts and
provenance. `diff.data_flow_comparison` compares compatible format-5/6/7 frozen captures using
normalization format 3. See [the diff contract](PYTHON_VALUE_DIFF.md). It reports
node syntax-fingerprint changes, local/cyclic value links and uniquely qualified
positional argument/return links independently of absolute source offsets.
Unsupported/incomplete coverage and legacy formats remain explicit. Frozen
candidate changes do not establish semantic behavior changes.

Independent query bounds: nodes 1..500, depth 1..30, a separate fixed 2000 physical-edge response cap, graph-candidate/traversal work
1..200000, 65536 states and 16 call frames. Work counts examined graph records,
bindings and transitions; parsing, snapshot loading, map construction and response
formatting are separate. Bounds and incomplete call boundaries are visible.
Capture is additionally bounded by 512 functions and 100000 syntax visits per
document, 20000 nodes/40000 edges per document, 4096 nodes/8192 edges/16384 work
units per function, 256 parameters/arguments, expression/branch depth 32 and 64 active control dependencies and 128 captured jump states per loop. Snapshot copies consume function capture work. Limits can
make capture unavailable; they never authorize an absence claim.

Current exclusions include exception type/filter evaluation proof, dynamic selectors/exception groups, nested/decorated/async
functions, methods, closures, nonlocal/global value reads, typed/default/variadic
parameters, keyword/spread arguments, mutation/heap aliases, containers, dynamic
calls and complete Python/operator/import/runtime semantics. Unknown function
forms are reported rather than connected with guessed value edges.
`qualified_subset_traversal_complete` describes only completion within the
captured bounded model. `full_program_slice`, `control_flow_modeled`,
`alias_analysis_complete`, `runtime_path_proven` and `absence_proven` remain false.
The optional full CPG backend and other language data-flow coverage remain open.

The read-only MCP tool requires `RAG_TOOLS=full`. Access classification,
telemetry and analysis-task usage attribution include it. `index_status` reports
the partial native Python backend while keeping full CPG unavailable. CLI, HTTP
role-specific qualification, representative quality/scale and runtime execution
coverage are not established by the focused stdio fixtures.

[Qualification](qualification/python-data-flow-2026-10-26/README.md).

[Branch qualification](qualification/python-branch-flow-2026-10-26/README.md).

[Loop qualification](qualification/python-loop-flow-2026-10-26/README.md).

Handler/else routing and alias cleanup: [contract](PYTHON_EXCEPTION_FLOW.md).
