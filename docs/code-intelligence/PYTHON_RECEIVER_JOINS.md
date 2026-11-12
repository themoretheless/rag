# Python local receiver-origin joins

Scope capture 5 / extraction format 89 records bounded reaching binding origins for local member reads. Statement `if/elif/else` joins live branches; an absent else retains the incoming state. Returned/raised branches do not feed later ordinary continuations. Simple local aliases copy their origins. Assignment RHS is evaluated before committing the new origin.

`while` and plain-target `for` compute a finite origin fixed point, including zero iterations and explicit continue backedges. Break environments feed loop exits separately; a loop else receives normal exit only. Iteration target types remain unknown unless assigned a subsequently captured value. Predicate truth, iteration count, termination and runtime feasibility remain unproved.

Each reaching origin is resolved through the existing project-scoped import/type/inheritance resolver using sparse reference/binding overrides. Candidate targets are deduplicated. Several receiver types can therefore resolve to the same inherited body without fabricating separate definitions. Ordinary single-origin results retain their previous resolution basis.

A known target plus an unknown origin is `ambiguous` even when the public candidate array has length one; its basis is `python_receiver_type_join_with_unknown`. It is not a uniquely proven target. No known targets means unresolved. Dead-code unknown-name roots and affected-test uncertainty accounting include partial-known references; they must not authorize skipping/deletion from the known subset. Runtime dispatch remains unverified.

The origin model covers captured local bindings and direct member expressions. It is not a full Python compiler, reachability oracle or heap/alias solver. Exceptional/other suites conservatively invalidate potentially written locals while retaining captures inside ordinary sub-suites; they are not a precise receiver-type exception model. Unsupported expression/definition/default/closure shapes can retain legacy conservative lookup. General destructuring type inference remains unknown; pattern writes to tracked locals invalidate their prior certainty. Constructor/annotation candidates do not prove successful initialization or runtime type identity.

Capture bounds: existing document scope/syntax limits, 512 function flows, 16384 work units per function, 4096 read locations, eight reaching origins per read/join, sixteen loop fixed-point iterations, bounded expression/assignment/region depth and 128 jump states per loop. Exhausted local flow makes affected tracked receiver references explicitly unresolved (`python_receiver_flow_capture_incomplete`). Missing flow capability payloads are not reused as current; source sync/indexing repairs them. These are logical caps, not physical memory/time or representative scale qualification.

Frozen graphs use `python_names_format=5` with explicit legacy/model-enrichment comparison flags. Value capture/normalization remain 7/3. Public source spelling, byte offsets, document/project/hash/revision and symbol IDs retain their original evidence. `index_status.capabilities.python_binding_evaluation` advertises the bounded joins and compiler proof limits.

[Qualification](qualification/python-receiver-joins-2026-10-26/README.md).
