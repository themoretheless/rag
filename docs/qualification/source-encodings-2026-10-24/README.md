# Source decoding and encoding evidence — 2026-10-24

Added strict UTF-8/UTF-16 BOM handling and deterministic, explicitly heuristic Windows-1251/1252 fallback to file extraction. Original byte hashes and decoded-source hashes are stored separately. Extraction-controlled metadata overwrites old encoding facts. Source sync upgrades missing metadata and refreshes encoding-only changes instead of treating identical decoded content as unchanged.

Search returns encoding facts only when their captured source hash matches the searched source. HTML keeps original decoded markup as structural evidence. Unsupported containers do not receive fabricated text encoding labels.

Verification:

- Decoder/extractor tests: 8 passed, including strict BOM handling, legacy Cyrillic/Western text, malformed input, binary rejection and HTML metadata refresh.
- Full library run after the implementation: 813 passed, 0 failed, 1 ignored.
- File ingestion integration: 3 passed.
- Final corpus regression includes stale encoding metadata rejection; see verification metadata.
- Actual stdio MCP encoding regression: `runtime.json` verifies Russian search/code symbols, both BOM forms, Western fallback, HTML source, warm no-extraction reuse and encoding-only source-sync refresh.
- Existing search/family-role MCP regression: `search-regression.json`.

The temporary test processes use mock embeddings and their own disposable database; production state is untouched. Heuristics remain uncertain, and live freshness/gate readers are not yet decoder-aware. See `docs/SOURCE_ENCODINGS.md`. This stage does not prove full RAGRAF transfer or universal encoding detection.
