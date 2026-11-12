# Python assignment binding order

Scope capture 4 / extraction format 88 defers activation of a plain assignment's inferred/annotated binding until the assignment expression ends. Its RHS therefore uses the preceding local binding; without one, a function-local read remains uninitialized rather than using the value being created. Module/class RHS lookup can still use the preceding global. Chained assignments share their RHS completion boundary, and NFKC spelling does not create another binding.

Annotation-only statements introduce an uninitialized function-local name but do not replace an already initialized parameter/local value. Module/class annotations alone do not initialize or shadow the name. Simple augmented assignments use the preceding binding for RHS evaluation and install an unknown result type afterward: the backend does not infer `__iadd__`/operator return type from the RHS constructor or preserve a stale receiver type as if it were proven.

Original public names, symbol IDs, byte offsets, hashes and revisions remain evidence. Format-3/missing scope facts are not reused as current; source sync/fresh indexing repairs them. Frozen graphs use `python_names_format=4`, preserving explicit old/new binding comparison availability and the analysis-enrichment warning for older captures. Value capture 7 and frozen value normalization 3 do not change in this milestone.

`index_status.capabilities.python_binding_evaluation` exposes the supported order/annotation/augmented-result behavior while keeping compiler/runtime verification false. This is a captured binding subset: branch-sensitive receiver unions, general destructuring/alias/type inference, attribute/subscript assignment effects, descriptors, exceptions, dynamic class behavior and complete compiler/runtime semantics remain open. An inferred constructor type does not prove that assignment succeeds or that later code is reachable.

Capture adds bounded assignment-commit (8192), augmented-write (2048) and annotation-only (2048) inventories within the existing bounded syntax walk. Incomplete capture yields explicit unresolved references, not an absence claim. Physical memory/time and representative corpus/quality qualification remain separate.

[Qualification](qualification/python-assignment-order-2026-10-26/README.md).
