# Decoder-aware live reads and gate freshness — 2026-10-24

The bounded, allowlisted live reader now uses the ingestion decoder for strict UTF-8, BOM UTF-8/UTF-16 and explicitly heuristic Windows-1251/1252. Freshness/live_read compare decoded source text, return separate observed byte/hash/encoding evidence, and keep immutable index history unchanged. Stored mode labels its UTF-8 representation explicitly.

Gate freshness shares the reader but charges original file bytes, including BOMs, toward its 16 MiB aggregate / 4 MiB per-file budgets. A smaller decoded representation cannot reduce the charged input size. Malformed declared encodings remain unavailable/unknown; no partial checksums qualify. Existing root, symlink, regular-file, size and concurrent-read checks remain before decoding.

Verification:

- Full library suite: 816 passed, 0 failed, 1 ignored.
- New live-source regression covers Windows-1251, UTF-16LE/BE and UTF-8 BOM, decoded reads, changed text, exact raw input budget and encoding-only raw-vs-text fingerprint differences.
- Gate regression verifies UTF-16 freshness, exact original-byte charging and malformed-BOM unknown status.
- Gate CLI build passed; `scripts/smoke_gate_calibration.py` passed through stdio, authenticated loopback HTTP and the actual hook executable (`gate-regression.json`).
- Actual stdio MCP test: `scripts/smoke_live_encodings.py`, report `runtime.json`. It additionally exercises search/source sync, encoding-only recoding, live reads, gate freshness and malformed input.

These checks use temporary files/databases and mock embeddings. They do not execute source files or proposed gate commands and do not modify production state. A fresh result proves decoded-text equality under the specified decoder, not historical byte identity or intended charset. Historical manifests retain text hashes; raw encoding-only changes are separately observable but do not create text-drift events. New filesystem-file discovery, additional encodings and platform/scale qualification remain outside this stage.
