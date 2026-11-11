# Captured source encodings

File ingestion and source synchronization decode supported text inputs before chunking and structural indexing. They preserve the decoded source for code/HTML analysis and capture byte-decoder evidence separately from searchable UTF-8 text.

Decoder version 1 uses deterministic rules:

1. UTF-8 BOM: strip the BOM and strictly decode UTF-8.
2. UTF-16LE/BE BOM: strip the BOM and strictly decode the declared byte order.
3. Otherwise accept strictly valid UTF-8.
4. Otherwise choose Windows-1251 when at least four Cyrillic byte candidates comprise at least 5% of ASCII/Cyrillic letters; use Windows-1252 as the Western fallback.

The last two legacy choices have explicit `heuristic_cyrillic_share` / `heuristic_western_fallback` bases. They are not proof of the author's intended encoding: other single-byte encodings and mixed-language content can be ambiguous. This is not universal encoding detection. Malformed BOM-declared input, UTF-32 and NUL-containing/binary-looking legacy text are rejected. No decoder inserts replacement characters for malformed input.

Captured metadata:

- `source_encoding`: canonical decoder label (`utf-8`, `utf-8-sig`, `utf-16le`, `utf-16be`, `windows-1251`, `windows-1252`).
- `source_encoding_basis`: BOM, strict UTF-8 validity, or named heuristic.
- `source_bytes_hash`: BLAKE3 of original bytes, including any BOM.
- `source_text_hash`: BLAKE3 of decoded UTF-8 source, before readable HTML extraction.
- `source_decoding_version`: 1.

For HTML, original decoded markup remains in the existing separate source-blob mechanism while document content stays readable text. Containers such as PDF/DOCX/XLSX are extracted by their format-specific readers and are not assigned a text-file source encoding.

`search_multi` returns captured encoding/basis/byte fingerprints only when versioned metadata matches the source text being searched. Manually inserted or changed documents with unknown/stale metadata return null. It never guesses the original encoding from already decoded text.

Source sync refreshes old documents without decoder metadata and updates decoder fields on encoding-only changes, even when decoded text is identical. Unchanged inputs retain the manifest fast path; its existing timestamp/size assumptions remain. The byte and decoded-text hashes describe different representations and must not be interchanged.

Live filesystem `freshness`, `live_read` and the gate now share this decoder. Checksums compare decoded UTF-8 source text; `observed_source` separately reports the byte count/hash, decoder label and basis. Encoding-only changes may preserve decoded freshness while changing the byte fingerprint. These observations do not prove byte identity with a historical input, because historical manifests retain text hashes rather than original byte hashes. `source=stored` labels evidence as `stored_utf8` / `stored_representation` and makes no live-byte claim.

Limits and gate accounting apply to original input bytes, including BOMs. Invalid/unsupported declared encodings remain unavailable/unknown. Heuristic choices remain explicit; a fresh result means equality under this deterministic decoder, not proof of the author's intended charset.

Qualification and reproducible MCP script: `scripts/smoke_source_encodings.py` and `docs/qualification/source-encodings-2026-10-24/`.
