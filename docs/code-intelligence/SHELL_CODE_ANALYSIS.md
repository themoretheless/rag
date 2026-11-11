# Bash and sh source analysis

The persistent code index recognizes `.sh` and `.bash` using the Bash tree-sitter grammar. It extracts function definitions, command calls, function exports and source dependency candidates. `.zsh` is not covered by this parser.

Root-level registrations follow source order. Calls before a definition are unresolved; unconditional redefinitions replace prior candidates. `unset -f` removes a known function. Definitions inside subshells and pipelines do not leak into the parent environment. Captured functions can override builtin names. `export -f` produces non-call function references as well as a command reference.

`source` and `.` link only literal absolute paths to fresh, captured shell documents in the same project. No filesystem or PATH search is performed. Relative paths and expansions are unresolved because source location does not establish the executing process's working directory. Recursive dependency traversal has a fixed budget and reports uncertainty when exhausted or cyclic.

Conditional definitions, late registrations inside function bodies, aliases, eval, traps and dynamic commands retain uncertainty. This is source analysis, not a shell interpreter: inherited environment functions, invocation arguments, options, recursive execution effects, alias parse-time behavior and runtime control flow are not established. Function-body registration timing is not proven. Results expose candidate bindings and limitations rather than claiming runtime dispatch.

SQL analysis accepts constant single-quoted and conservatively decoded double-quoted literals. It never evaluates expansions, substitutions or scripts. SQL-looking strings whose values are unverified produce a diagnostic instead of guessed table names.
