# Declared enum/domain/set types: local qualification, 2026-10-24

The native database block now captures declared PostgreSQL enum/domain chains and
MySQL enum/set members, integrates these facts into drift, and resolves supported
domain identifiers without dropping their constraints. This is a locally verified
subset, not completion of the full RAGRAF transfer/improvement goal. See the
[contract](../../EXTERNAL_DATABASES.md) and [matrix](../../RAGRAF_FEATURE_MATRIX.md).

## Actual verification

- `cargo test --lib`: **759 passed, 0 failed, 1 ignored**. The ignored fixture test
  was separately provisioned and executed successfully on real PostgreSQL 16 and
  MySQL 8.4 in uniquely owned disposable Docker containers.
- `scripts/smoke_external_db.py`: the actual binary build, native fixture and real
  stdio/HTTP MCP exercise passed. No production database or existing RAG server
  was configured or restarted. Owned containers/volumes were removed afterwards.
- PostgreSQL enum labels preserve order, comma/apostrophe/backslash values and
  identity by exact schema/name. Used array/domain dependencies are captured without
  persisting OIDs or internal enum sort values. Enum arrays expose their element
  type. Basic built-in range/subtype metadata remains usable by the earlier fixture.
- A nested integer domain retains both domain definitions, their defaults,
  NOT NULL and CHECK/validation metadata. The owning column resolves to integer
  for naming inference while the domain chain and unevaluated-constraint warning
  remain available. The native fixture has five naming hypotheses instead of four:
  `domain_user_id` can now join the same scalar-ID candidate family.
- A domain default 9 is observable in an explicitly allowed row sample. A nullable
  array of a NOT NULL/defaulted element domain stays nullable and gets no fabricated
  array default. Both synthetic type-chain tests and real catalog assertions cover
  this boundary; fixture insertion omits that array column successfully.
- MySQL ENUM and SET retain ordered member lists, charset/collation and special
  characters; they are not split on commas. Unit parser cases include empty values,
  Unicode and rejection of unquoted/hex/extra-SQL inputs. Unsupported literals and
  bounded/truncated definitions produce incomplete metadata, not invented members.
- The real MySQL mode probe reproduces the distinction between canonical metadata
  and reader SQL syntax. It exercises normal creation/normal read, normal
  creation/NO_BACKSLASH_ESCAPES read, and NO_BACKSLASH_ESCAPES creation/read using
  a temporary probe table. Normalized definitions stay equal; backslashes are
  neither dropped nor doubled. The owned fixture's global mode is restored.
- Public collection remains a read-only transaction. Only test-fixture setup uses
  administrative DDL. Row sampling remains opt-in with explicit owner columns;
  known enums, scalar domains, SETs and supported arrays can be displayed under
  those controls. No sample contains the excluded fixture private-row marker.
- Both native connectors reject an 8 MiB + 1 text-result probe with the budget
  error. Metadata is consumed as a stream, and table snapshots are size-checked
  incrementally; no oversized result is published as a successful capture.

The MCP scenario applies **independent real DDL steps**. PostgreSQL first adds an
enum value with identical column metadata before/after; drift identifies exactly
`events` from its type catalog. A separate step changes the domain CHECK validation
and default; the next capture exposes the new value/default and validation flag.
MySQL ENUM and SET additions are separate steps, each detected on exactly `events`.
Original captures retain the old member lists. Earlier index/CHECK/view and identity
mode changes, HTTP Read/Write/Admin rules, failed-refresh retention and frozen
naming hypotheses remain covered by the same end-to-end run.

Evidence: [runtime.json](runtime.json), [verification.json](verification.json),
[source-sha256.json](source-sha256.json). Assertions are in
`src/external_db/{mod,types}.rs` and `scripts/smoke_external_db_runtime.py`.

## Compatibility and limits

Capture format 4 / structural format 3 requires a declared-type catalog. A unit
case loads an actual stored format-3-shaped payload without it, verifies the
stored bytes remain unchanged after a read, and verifies the first richer capture
is refused as a physical-DDL comparison against the old format. Older frozen
inference is not recomputed. New captures use inference algorithm 2, including
supported domain-base and enum identifier comparisons.

The MySQL canonical renderer calls are visible in the pinned
[MySQL 8.4 source](https://github.com/mysql/mysql-server/blob/mysql-8.4.0/sql/field.cc);
this supports the decoder rule, while the native mode probes provide runtime
verification. No vendor code was copied into the collector.

This does not qualify every type, database version, SQL mode or charset. Composite
and opaque/custom type implementations, function bodies, spatial parameters,
complete sequence definitions, labelled inference precision/recall, trusted-CA TLS,
large-schema pagination/performance and full reference parity remain open.
Definitions are bounded at 16 type edges, 256 definitions, 512 members and 256
domain constraints per used type scope, with explicit diagnostics/hashes on
truncation. Constraint capture is not constraint evaluation against proposed rows
and is not a guarantee that a live INSERT succeeds. Schema literals can themselves
be confidential and remain subject to the existing authorized snapshot-reader scope.
