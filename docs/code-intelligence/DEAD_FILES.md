# Stored-source file candidates

Use dead_code with `dead_code_view=files`; the default routines view is unchanged.
`dead_code_project` selects an exact stored wing/project, and `path` optionally
selects an exact candidate path. Literal referrers are still searched throughout
the selected project(s), outside the candidate path. No live source file is read,
source program runs, external connection opens or code is deleted.

Known incoming cross-document bound/structural links, known entrypoints and
shebang prefixes protect files before literal checking. `protected` reports those
reasons. For the remaining files, exact-case literal text mentions can refute a
candidate. Spellings include the stored path, a path relative to the common
stored project parent, and a basename only when unique in that project. Adjacent
letters/digits/underscore/dash/dot block substring matches such as file.py.bak.
No URI decoding, escapes, cwd aliases or dynamic path construction is inferred.
Comments and strings can refute a candidate: a mention does not prove usage.

`refuted` supplies the referring document/path/project, revision, decoded-source
hash, line/byte offsets and matched needle, with binding_proven=false. Source text
and snippets are omitted. Preserved original HTML is used, not just readable text.
Matches never cross project identity and self-document mentions are excluded.
Duplicate basenames are not guessed. No filesystem identity/alias check is made.

`items` contains possible file candidates only after the configured checks have
completed without a mention. `not_checked` identifies candidates blocked by scan
limits or unavailable source/graph facts. Positive refutations remain useful even
when other sources could not be checked. Each category has an independent bounded
window, count and truncation flag. `files_fully_checked` means the captured graph
subset plus configured literal spellings were checked; it does not mean complete
language/runtime dependency coverage. Unresolved module/file imports, reflection,
resources and external usage remain unknown. deletion_safe/runtime_usage_proven
are always false; graph_extraction_exhaustive is false.

Candidate checks default to 100/cap 500 (`dead_code_max_candidates`). Literal body
reads default to 2000/cap 100000 (`dead_code_max_documents`), and decoded body bytes
default to 8 MiB/cap 64 MiB (`dead_code_max_bytes`). One source is capped at 2 MiB;
compiled literal patterns at 2 MiB. Oversized/unavailable/skipped sources prevent
negative classification for that project. A stale/unindexed/parse-error graph or
no published revision also prevents negative classification. Positive mention
proofs may still be returned. Unpreserved HTML cannot establish absence.

The literal budgets do **not** bound the preexisting checksum-validated graph
loading, source-file metadata enumeration, DuckDB internal IO, total CPU or RSS.
That separate phase is explicitly excluded in the response. All phases share one
read transaction, so concurrent ingestion cannot mix evidence within a result.
Only active stored documents with source_file participate in literal checks;
wiki documents without source-file identity are not scanned. The query requires
no schema/extractor migration. Representative scale/quality is not qualified.
