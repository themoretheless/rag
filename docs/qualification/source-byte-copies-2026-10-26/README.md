# Captured source-byte copies — 2026-10-26

Implemented [source-byte duplicate contract](../../SOURCE_BYTE_DUPLICATES.md).
The new grouping separates captured source-byte identity from decoded/extracted
text equality, including supported binary document containers.

**1,435 workspace tests passed, 0 failed, 1 ignored**; Clippy with `-D warnings`,
owned-source formatting, Python syntax and `git diff --check` passed. Six new
native cases cover capture validation, text/byte distinction, project and
inconsistent-length guards, stale metadata exclusion, UTF-8 BOM identity,
DOCX/XLSX capture, stable extraction after changing the original file, normal
snapshot cleanup and requested-source error paths. The initial ingest log retains
a failed XLSX fixture missing workbook XML; the fixture was corrected and all
six new cases passed in the final workspace run. The 42-case corpus run also
passed; it predates the final sync-preflight fingerprint enhancement.

An isolated rebuilt MCP binary passed the new runtime scenario on 16 project
files plus a foreign-project control. Expected raw-copy member sets are computed
from SHA256 of the actual input bytes (an independent oracle for the BLAKE3
partitions). Six source-byte groups cover UTF-8, UTF-16LE, Windows-1251, DOCX,
XLSX and actual PDF extraction through the installed pdftotext. UTF-8 BOM,
UTF-16BE and differently compressed Office archives are controls: text may match
while original-byte identity differs. Query replies remain body-free.

The same scenario verifies source digest/count provenance, project/prefix scope,
determinism, stored observations despite live file changes, BOM-only sync
replacement, legacy metadata recapture, mismatched-text metadata repair,
member/group caps and independence from normalized-line comparison budgets.

Seven additional real MCP regressions passed on that same executable: numbered
families, primary comparisons, duplicate roles, search roles, literal path roles,
SQL table impact and file imports. All tests use temporary source files/databases
and mock embeddings. Ingested source/SQL is never executed; PDF extraction reads
only the generated local fixture. Production data is untouched.

Runtime reports store schema **25**, extraction format **53**; the latter was
advanced by concurrent shared-tree work. This metadata/query extension itself
does not add an extraction-format version. Existing tempfile dependency was
promoted from development-only to runtime for secure capture cleanup. Capture
uses a 64 KiB copy buffer, but this is not a whole-query memory or scale bound.

`verification.json` records executable/log/source hashes and detailed outcomes.
This is qualification of the checked shared-tree build and isolated executable,
not a frozen checkout reconstruction. Full parity, arbitrary unsupported binary
ingestion, current filesystem equality, semantic/deletion safety, crash cleanup,
representative performance and broad platform qualification remain open.
