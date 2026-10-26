# Static file import candidates

File impact (`impact_file` or `impact_document_id`, default `references` mode)
includes separate `file_import_candidate` witnesses. An imported file can have
no indexed symbols, or its requested export can be missing. This relationship
never creates a call edge or makes all routines in the module live. `calls` mode
excludes it. The files view of `dead_code` protects targets of these relations.

Extraction covers Python `import`/`from`, including aliases, relative modules,
possible imported submodules and captured package initializers. JS/TS covers
static ESM imports (including side-effect imports) and re-exports. Candidates
come only from fresh, parseable captured documents in the same project and exact
normalized module paths. All alternative Python enclosing roots or JS/TS source
extensions remain candidates; ambiguity is explicit. Witnesses retain source
revision, content hash, statement line/offset, specifier and extraction kind.

These are conservative syntax candidates, not proven loader/runtime behavior.
Python sys.path, installed/namespace packages, runtime import hooks, ESM package
exports/tsconfig aliases, CommonJS require, dynamic imports, encoded/escaped
specifiers, resources and other languages are not resolved by this collection.
Python `from module import name` may refer to a value or a submodule: its optional
submodule relationship is labelled as a candidate. Type-only or conditionally
executed static imports may still protect a file. Case follows stored paths.
JS/TS extension alternatives include source/transpilation candidates, not a
claim that a particular loader would select them. Unresolved specs have no
traversable target; empty impact never proves absence of runtime use.

The extraction format advances to 34 (store schema unchanged). Indexing/sync
replaces cached facts. Stale/unindexed/parse-error documents cannot bridge file
impact. Frozen revision graphs store file imports; `diff` reports separate
`file_import_candidate` relations by document identity. If either historical
revision lacks this collection, `file_import_relations_available=false` and no
additions/removals are fabricated for this relation family.

Qualification: [file-imports-2026-10-25](qualification/file-imports-2026-10-25/README.md).

HTML/CSS file resources now have a separate [resource collection](FILE_RESOURCE_RELATIONS.md), introduced in format 35. The loader/asset coverage limits of that collection remain explicit.
