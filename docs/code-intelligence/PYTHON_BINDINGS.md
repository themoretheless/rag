# Python identifier bindings

The internal resolver normalizes Python identifiers with NFKC while preserving original symbol IDs, source spelling, byte offsets, revisions and content hashes in public evidence. Python resolution uses only Python documents in the selected projects. Extraction format 89 invalidates older binding caches and missing scope captures are rebuilt on fresh reads.

Captured function body ranges hoist local assignments into their function scope. Parameters retain their initial type before a later assignment. Class dictionaries are visible in class-body evaluation and method default expressions, but do not form lexical closures over method bodies. Unbound `self` is not inferred from its spelling. A captured binding named `super` prevents builtin-super inference.

Lambda/comprehension ranges and functions/modules containing unsupported global/nonlocal, complex iteration targets/handler aliases, with, deletion or assignment-expression binding forms conservatively return unresolved candidates. These guards deliberately sacrifice some valid links. Dynamic dispatch, complete Python compiler semantics, module execution order/redefinitions, runtime decorators and comprehensive alias/type inference remain unqualified.

Plain `for` identifier targets now capture unknown local bindings, hoisted in function scope and initialized at the body boundary; they shadow global calls while unrelated imported functions remain resolvable. Compatibility spellings use the same normalized key. Scope capture format 5 repairs older/missing facts.

Frozen index graphs record `python_names_format=5`; diffs expose availability and explicitly identify legacy changes that may be analysis enrichment. They do not prove source semantic changes, and legacy edge deltas are not suppressed.

Qualification: [2026-10-26](qualification/python-bindings-2026-10-26/README.md). Fixtures are analyzed as text only; the MCP test uses temporary databases, mock embeddings and disabled LLMs.

Handler aliases use the shared Python AST adapter, initialize at the alias token end and clear at the handler end. Unknown cleanup bindings mask stale handler-local inferred values; unrelated imports remain available. See [exception binding qualification](PYTHON_EXCEPTION_FLOW.md).

Assignment bindings now activate after RHS completion; annotation-only function locals and unknown augmented-assignment results are captured separately. See [assignment order](PYTHON_ASSIGNMENT_ORDER.md).

Bounded local receiver origins now join ordinary branches and loop fixed points. Partial-known references remain ambiguous and retain unknown-name liveness guards. See [receiver joins](PYTHON_RECEIVER_JOINS.md).
