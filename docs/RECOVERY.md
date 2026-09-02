# Backup and recovery

Recovery filesystem paths must be beneath `RAG_INGEST_ROOTS`. Existing files are never replaced unless `overwrite=true`; imports default to `dry_run=true`.

- `backup_db(path, dry_run?, overwrite?)` briefly holds the Store mutex for `CHECKPOINT` and cloning a dedicated snapshot connection. A read transaction on that connection pins one source MVCC generation, then `COPY FROM DATABASE` copies schema and data into a staged DuckDB database without retaining the shared mutex. The staged database is verified before publication. Publication is serialized per destination: all three artifacts are staged and synced, the sidecar public paths are installed first, and the verified database path is published last as the generation commit marker. This is a coordinated three-file publication, not a claim that the filesystem can rename three paths atomically. Normal Store queries remain available during the potentially long database copy.
- `export_bundle(path, format?, dry_run?, overwrite?)` writes portable `json` or line-delimited `jsonl` containing documents, document metadata, stable IDs/timestamps, chunks, and the canonical corpus `embedding_manifest` in recovery-bundle format v2. A vector-bearing export is refused if the manifest is missing, non-canonical, marked as an incomplete migration, or disagrees with a chunk's dimensions.
- `export_vault(path, dry_run?, overwrite?)` writes readable Markdown under `projects/<wing>/<room>/<layer>/` and machine-readable `.rag/graph.json`, `.rag/ops-log.json`, and `.rag/manifest.json`. It defaults to dry-run. Explicit overwrite moves the previous vault to a dated `.previous-*` sibling before publishing the new one.
- `import_bundle(path, format?, dry_run?, conflict_policy?, reembed_legacy?)` restores a bundle transactionally. Conflict policy is `error` (default, rolls back), `skip`, or explicit `overwrite`. A v2 bundle with chunks must carry one canonical `embedding_manifest`; its provider, model, dimensions, base URL, and content fingerprint must exactly match a non-empty target corpus. An empty target adopts the bundle identity on apply.

Recommended flow: always create and verify `backup_db`; when the corpus fits the
portable limits below, also export a bundle, dry-run `import_bundle`, inspect
counts/conflicts/errors and embedding identity, and only then apply to the
intended target. Export a vault when human-readable/Git interoperability is
useful. A DuckDB backup restores the complete store. A portable bundle restores
documents and retrieval chunks; derived graph/index data can be rebuilt with
existing maintenance tools. The Markdown vault is intended for human
inspection, Git history, and interoperability rather than lossless database
restore.

## Portable bundle bounds

Portable JSON/JSONL is a bounded in-memory interchange format, not the large
corpus backup path. One bundle is limited to 64 MiB of input/encoded output,
10,000 documents, and 50,000 chunks. Before export, DuckDB aggregates counts
and a conservative size estimate without loading document bodies or vectors;
the in-memory bundle is checked again before encoding, and the encoded byte
count is checked before atomic publication. Import checks file metadata before
reading, reads through a 64 MiB bounded reader to close file-growth/replacement
races, and validates counts and the materialized estimate before any database
mutation. A rejected export leaves no new destination artifact and does not
replace an existing one.

For a corpus above those bounds, use `backup_db` or offline `recovery backup`,
then require `recovery verify` (or the generated verification metadata) to pass
the checksum, schema, relational-integrity, and embedding-contract checks before
restoring that DuckDB backup. Do not split or hand-edit vectors merely to bypass
the portable limit.

## Bundle versions and embedding identity

Recovery bundle **v2** makes vector provenance part of the recovery contract.
For both JSON and JSONL, the serialized `embedding_manifest` is the singleton
`default` identity over provider, model, dimensions, base URL, and the canonical
content fingerprint. JSONL has exactly one manifest header before its document
records; empty, headerless, duplicate-header, unsupported-version, and malformed
streams are refused rather than guessed.

Legacy v1 bundles did not prove which embedding identity produced their stored
vectors. The running gateway therefore handles them explicitly:

- A metadata-only v1 bundle (no chunks) is safe to upgrade in memory and import.
- A v1 bundle containing chunks is refused unless the MCP `import_bundle` call
  sets `reembed_legacy=true`. The gateway then replaces **every** legacy vector
  with the live embedding provider in bounded batches before the transactional
  import. Start with `dry_run=true`; its report shows
  `legacy_bundle_version`, `embeddings_reembed_planned`, and zero actual
  `embeddings_reembedded`. The apply report distinguishes vectors actually
  regenerated and whether document/chunk work committed with
  `durable_mutation_committed`.
- The offline `recovery import-bundle` CLI deliberately refuses vector-bearing
  v1 bundles because it has no live embedding provider. Use the single running
  gateway with `reembed_legacy=true`; the CLI can still import metadata-only v1.

Do not manufacture a v2 header or copy an arbitrary manifest onto legacy
vectors. That would make a bundle syntactically current while leaving retrieval
semantically unverified.

## Missing manifest on an existing corpus

Startup never self-certifies already stored vectors. If chunks exist but the
`embedding_manifest` is missing, the gateway remains available for status,
diagnosis, and repair, while vector-producing and vector-consuming operations
stay fail-closed and `ready_for_search` stays false. Repair the corpus with one
complete uncapped `reembed_all` under the intended live embedding configuration.
Raise `RAG_MAINT_MAX_DOCS` to at least the corpus document count before
restarting the gateway; per-call `max_docs` cannot exceed that configured cap.
Only a run that covers the whole corpus with no skipped or failed documents may
publish the verified target manifest; a partial run is not recovery.

## Derived-index finalization after import

An applied import owns the corpus mutation lane through eager FTS finalization.
If document/chunk changes commit but that finalization fails, the aggregate
report is unsuccessful, retains its committed counts, sets
`durable_mutation_committed=true`, and includes `FTS_FINALIZATION_FAILED` with
`retryable=true`. The store keeps or rewrites the crash-safe dirty marker so the
next lexical/hybrid read can retry FTS once. Inspect the report before deciding
whether replay is safe; do not blindly repeat an already committed import.

## Offline recovery CLI and RTO drill

Stop the live gateway before opening its database with the offline CLI. The target recovery time for a local restore is 15 minutes after a usable backup has been selected.

1. Stop the single-writer service and record the live database path.
2. List candidates with `cargo run --bin recovery -- inventory --dir backups`.
3. Verify the chosen snapshot with `cargo run --bin recovery -- verify --backup backups/FILE.duckdb`.
4. Exercise the complete copy and comparison with `cargo run --bin recovery -- restore-drill --db LIVE.duckdb --out backups/drill.duckdb`. The drill compares row counts, schema version, relational integrity, and the embedding manifest.
5. Preserve the damaged live file, copy the verified snapshot into a new path, point `RAG_DB_PATH` at that path, and start the gateway.
6. Require `/live` and `/ready` to return success, then run `doctor` and one known search before declaring recovery complete.

`recovery backup` writes `<file>.sha256` and `<file>.metadata.json`; the metadata includes verification results and a conservative free-space requirement of twice the snapshot size. `recovery retention` is preview-only: it never deletes files and always protects the newest snapshot plus files whose names contain `final`. Bundle imports remain dry-run unless `--apply` is supplied.
