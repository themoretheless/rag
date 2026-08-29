# Backup and recovery

Recovery filesystem paths must be beneath `RAG_INGEST_ROOTS`. Existing files are never replaced unless `overwrite=true`; imports default to `dry_run=true`.

- `backup_db(path, dry_run?, overwrite?)` checkpoints DuckDB while holding the store lock, then copies the consistent database file.
- `export_bundle(path, format?, dry_run?, overwrite?)` writes portable `json` or line-delimited `jsonl` containing documents, document metadata, stable IDs/timestamps, and chunks.
- `import_bundle(path, format?, dry_run?, conflict_policy?)` restores a bundle transactionally. Conflict policy is `error` (default, rolls back), `skip`, or explicit `overwrite`.

Recommended flow: create `backup_db`, export a bundle, run `import_bundle` with `dry_run=true`, inspect counts/conflicts/errors, then repeat with `dry_run=false`. A DuckDB backup restores the complete store. A portable bundle restores documents and retrieval chunks; derived graph/index data can be rebuilt with existing maintenance tools.
