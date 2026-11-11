# Possible reachability for dead-code candidates

`dead_code` returns routines outside a conservative possible-reachability set.
Every candidate keeps deletion_safe=false, candidate_only=true and
external_usage_unknown=true. It does not prove execution, nonexecution or safe
deletion. Its reason is outside_possible_static_reachability.

Roots include known entrypoints, module-level candidate calls and all bound
non-call targets (callbacks, type/member values). Bound references identify exact
candidate symbol IDs; their short names do not suppress unrelated definitions.
For unresolved calls or values, same-name routines in the same stored project
are possible roots. Project identity, not path spelling, restricts this fallback;
unscoped documents share the existing unscoped project identity.

Possible roots propagate through candidate calls. A possible-live base member
also reaches its stored nearest inherited member candidates and their callees.
This is a conservative exclusion from dead-code candidates, never a synthetic
call or proof of virtual dispatch. All symbol arcs enforce equal project identity.
Cycles terminate through a visited set. Results sort by source path, document ID,
line and symbol ID before applying the existing result cap.

Detail fields report reachable_symbols, unresolved_name_roots (newly added
possible symbol roots), override_candidate_edges (available guarded arcs),
lexical_fallback_scope=same_project_unresolved_names_only and
runtime_dispatch_proven=false. Using unresolved-name roots marks analysis
incomplete. Existing stale/unindexed/parse/binding diagnostics remain visible.
Unsupported frameworks, reflection, dynamic dispatch/imports and external usage
remain unknown. Inherited member candidates currently cover their documented
Python/JS/TS syntax subset; other language dispatch is not inferred.

The query uses existing captured facts; no database or extractor migration is
needed, no source application runs and no code is deleted. Selecting a path
filters candidate output, not the global stored-project reachability roots.
