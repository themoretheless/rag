# Backup and recovery

Recovery filesystem paths must be beneath `RAG_INGEST_ROOTS`. Existing files are never replaced unless `overwrite=true`; imports default to `dry_run=true`.

- `backup_db(path, dry_run?, overwrite?)` checkpoints DuckDB while holding the store lock, then copies the consistent database file.
- `export_bundle(path, format?, dry_run?, overwrite?)` writes portable `json` or line-delimited `jsonl` containing documents, document metadata, stable IDs/timestamps, and chunks.
- `export_vault(path, dry_run?, overwrite?)` writes readable Markdown under `projects/<wing>/<room>/<layer>/` and machine-readable `.rag/graph.json`, `.rag/ops-log.json`, and `.rag/manifest.json`. It defaults to dry-run. Explicit overwrite moves the previous vault to a dated `.previous-*` sibling before publishing the new one.
- `import_bundle(path, format?, dry_run?, conflict_policy?)` restores a bundle transactionally. Conflict policy is `error` (default, rolls back), `skip`, or explicit `overwrite`.

Recommended flow: create `backup_db`, export a bundle and vault, run `import_bundle` with `dry_run=true`, inspect counts/conflicts/errors, then repeat with `dry_run=false`. A DuckDB backup restores the complete store. A portable bundle restores documents and retrieval chunks; derived graph/index data can be rebuilt with existing maintenance tools. The Markdown vault is intended for human inspection, Git history, and interoperability rather than lossless database restore.

## Offline recovery CLI and RTO drill

Stop the live gateway before opening its database with the offline CLI. The target recovery time for a local restore is 15 minutes after a usable backup has been selected.

1. Stop the single-writer service and record the live database path.
2. List candidates with `cargo run --bin recovery -- inventory --dir backups`.
3. Verify the chosen snapshot with `cargo run --bin recovery -- verify --backup backups/FILE.duckdb`.
4. Exercise the complete copy and comparison with `cargo run --bin recovery -- restore-drill --db LIVE.duckdb --out backups/drill.duckdb`. The drill compares row counts, schema version, relational integrity, and the embedding manifest.
5. Preserve the damaged live file, copy the verified snapshot into a new path, point `RAG_DB_PATH` at that path, and start the gateway.
6. Require `/live` and `/ready` to return success, then run `doctor` and one known search before declaring recovery complete.

`recovery backup` writes `<file>.sha256` and `<file>.metadata.json`; the metadata includes verification results and a conservative free-space requirement of twice the snapshot size. `recovery retention` is preview-only: it never deletes files and always protects the newest snapshot plus files whose names contain `final`. Bundle imports remain dry-run unless `--apply` is supplied.
