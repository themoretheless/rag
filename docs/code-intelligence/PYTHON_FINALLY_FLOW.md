# Python completion routing through finally

Value capture format 7 / extraction format 87 adds `try/finally` to the native Python `data_slice` model. The builder separates the already evaluated return value from the local environment carried to finalization. Normal completion preserves the pending return/break/continue; an explicit completion in `finally` overrides it. Break/continue carry the updated local environment to the target loop. Nested finalizers route the updated state outward.

The backend also captures conservative pre-operation error states for arithmetic/comparison/unary operations, direct positional calls, condition truth tests and iterator protocol steps inside protected try bodies. Explicit `raise expression` supplies an exceptional completion. Each prefix retains the environment before a possibly failing assignment; `finally` can therefore read the preceding definition. Expression-error types, occurrence probabilities and feasibility are not proven. Literal truth tests and parenthesis wrappers do not create additional error events. Errors in a finalizer are routed to an enclosing protected region where one exists; escaping exceptional routes do not become successful return seeds.

This is not complete exception analysis: exception groups, dynamic type selectors, bare raises without captured exception context, chained raises, asynchronous exceptions, resource failures, arbitrary iterator/operator/heap effects and full type matching remain unsupported/unproven. A finally read uninitialized on a captured input path makes the function unavailable. An always-raising completion can leave no captured successful return; backward `data_slice` then reports `seed_unavailable`, without proving runtime absence.

One physical finalizer may run for several incoming completions. Its nodes carry `flow_context` labels and context-specific IDs. Call frames use the result occurrence including that context, preventing value exchange between separate finalizer invocations. Local values that differ across incoming completions receive `completion_binding` proxies with `completion_value` and `completion_control` edges. Unchanged local values and constant finalizer returns do not acquire unrelated pending-return/error dependencies. Original return operands remain evaluated before finalization; assigning their local name in `finally` does not rewrite the saved return value.

Variable selectors still select physical definition/parameter tokens. Several contexts of the same token are seeded together; distinct writes remain ambiguous unless `start_byte` selects a single physical occurrence. Node and edge unions remain syntactic may-candidates, not one feasible runtime path. Source byte spans, hashes/revisions and per-call witness/edge context checks remain intact.

The existing independent work/node/depth/edge limits apply. New capture limits include 128 error-prefix states per protected region, 128 incoming completions per finalizer, and bounded finalizer nesting/context depth. Environment copies consume function capture work. Exhaustion clears unsupported function facts and remains explicit. This is not a physical memory/time bound or large-corpus performance qualification.

Fresh indexing repairs older value captures; read-only data queries do not mutate caches. Frozen diff normalization format 3 supports capture formats 5, 6 and 7, includes completion context in normalized identity and guards old unsupported finalizer coverage. Ordinary old/new plain-function graphs can compare without source-position churn. Context ordinals are not a semantic/minimal-edit matcher.

`index_status` and query metadata advertise finalizer routing and protected error prefixes while keeping type/filter proof, full exception semantics, complete CFG, runtime path proof, termination, heap/alias completeness and full CPG unavailable/unproven.

[Qualification](qualification/python-finally-flow-2026-10-26/README.md).

Handler/else extensions and cross-format context comparison guards: [contract](PYTHON_EXCEPTION_FLOW.md).
