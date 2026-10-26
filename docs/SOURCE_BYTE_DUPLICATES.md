# Captured source-byte copies

`duplicates(grouping="source_byte_copies")` inventories partitions of supported
ingested files by **captured BLAKE3 source-byte digest and byte count**, scoped
by project and component path prefix. The grouping ignores names and decoded
text similarity. Groups require two qualified captures. Token grouping is
skipped; `min_similarity` does not filter source-byte partitions.

Every returned member, in all duplicate modes, now includes
`source_bytes_to_primary` independently of normalized-line comparison budgets:

* `same_capture_digest`: both qualified captures have equal digest and length;
* `different_capture_digest`: qualified source-byte digests differ, even if the
  decoded/extracted text is identical;
* `inconsistent_digest_byte_count`: equal digests but different recorded lengths;
  equality is unknown, and those captures cannot share a byte-copy group;
* `unavailable`: one or both captures lack qualified evidence. Equality is null.

This is cryptographic capture evidence, matching the reference's source-hash
partition semantics. It is not a direct byte-by-byte comparison, current disk
read, signed attestation, semantic equivalence or deletion recommendation.
Original bytes are not permanently retained. `same_decoded_text` remains a
separate observation. A BOM, UTF-16 byte order or different ZIP compression can
preserve text while changing source-byte identity.

## Capture and validation

File ingestion records `source_capture_version: 1`,
`source_bytes_hash_algorithm: blake3`, `source_bytes_hash`, `source_bytes_count`
and `source_text_hash`. The last binds the capture to the decoded/extracted text
used for corpus queries. HTML uses preserved original markup, not its readable
projection. Existing decoder evidence remains separately versioned.

Text hashes use exactly the bytes passed to the decoder. PDF/DOCX/XLSX are
stream-copied into a secure temporary file while hashing/counting input. Their
existing extractors read that same captured temporary file. A source changing
after capture cannot make extraction silently use different original bytes.
The temporary file is deleted on normal scope exit, including extraction errors.
Source errors identify the requested path. This does not guarantee an atomic
filesystem snapshot while a concurrent writer mutates the original during the
copy, or cleanup after an uncatchable process crash.

`document.source_byte_capture` qualifies metadata only when the version,
algorithm, lowercase 64-hex digest, unsigned length and source-text fingerprint
are valid. Missing/legacy metadata, unsupported versions/algorithms, malformed
hashes/lengths and mismatched text retain explicit availability states. There is
no fallback from a decoded-text hash to a supposed raw-byte hash. Query-level
`source_byte_identity.capture_status_counts` inventories availability even when
no group is returned. Metadata originated outside ingestion is not authenticated.

Source sync recognizes old/missing capture metadata and re-extracts it even if
the manifest stamp is unchanged. Capture fields participate in attachment
comparison, so a BOM-only change or other byte change with identical decoded
text updates evidence. Preflight also checks the text fingerprint against the
stored body or preserved HTML source. This is a one-time recapture for legacy
sources, with no extractor-format bump required.

## Scope and limits

The source-byte view is opt-in. Existing `similarity_components` and
`exact_and_name_families` remain available. The response preserves primary role
ranking, body-free identities, independent group/member windows and total counts.
Source-byte metadata work is outside normalized-line comparison budgets; disabling
that phase does not disable byte-copy groups. Corpus loading, temporary disk IO
and total query memory do not gain a new global budget here.

Supported text, PDF and OOXML ingestion paths are covered. Arbitrary binary
assets/formats not accepted by ingestion, live disk validation, full failure and
platform qualification, representative throughput/memory and duplicate quality
remain open. [Qualification](qualification/source-byte-copies-2026-10-26/README.md).
