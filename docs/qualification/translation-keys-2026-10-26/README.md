# Translation-key references — 2026-10-26

Implemented [captured key syntax, queries and immutable history](../../TRANSLATION_KEY_REFERENCES.md).
Full RAGRAF parity and translator/template runtime binding remain unproven.

**1,460 workspace tests passed, 0 failed, 1 ignored**. Clippy with `-D warnings`,
owned-source formatting, Python syntax and `git diff --check` passed. Nine new
native cases cover placeholders/repeats/UTF-8 anchors, PHP literal-call owners
and dynamic exclusions, quote/escape decoding, 120-character Unicode limits,
source/occurrence limits, project/count/stale guards, missing capability repair,
position/count/legacy history, and exclusion of incomplete captures. The selected
10-case run also includes one existing C translation-unit test. Final workspace
includes the 120-Cyrillic/CJK/four-byte-character controls.

Early logs retain a temporary AST iterator-borrow compile error and unrelated
in-progress Rust re-export module/lifetime errors. They were resolved before the
final gates. Initial subset/pre-Unicode-limit runs are retained for traceability;
`workspace.log`, `clippy.log`, `build.log`, `format.log` and `runtime.json` are the
final evidence, not the earlier logs whose names contain `final`.

The real MCP scenario verifies shared template/PHP key lookup, namespace/case
translator-name candidates without claiming bindings, PHP/PHTML owners/file
origins, exact case/space identity, a 120-CJK-character key, excluded dynamic,
interpolated/composed/named/member calls, decoded static escapes, UTF-8 source
anchors, repeated occurrences, project ambiguity before path filters, independent
result limits, the 4,096-fact cap, invalid/mixed-target errors, stale exclusion,
actual sync replacement and immutable history. Position changes and equivalent
`reg.months` / `reg\x2Emonths` literal spelling produce no key relation churn.
A deliberately incomplete capture is explicitly excluded from relation diff.

The pinned reference revision is `075caf86dc2f6274b9c48487e521915bcdfeaf2c`.
Only its placeholder regex and pure `placeholders_from_text` function execute on
a fixed template; the resulting key/line sets match local facts. Local capture
additionally preserves repeated same-line occurrences and byte spans. This is
finite contract evidence, not a representative quality benchmark. PHP escape
semantics were checked against the [official string manual](https://www.php.net/manual/en/language.types.string.php);
no PHP runtime or ingested code/SQL executes.

Six additional MCP scripts passed on the same isolated final executable: SQL/URL
references, PHP routes, markup endpoints, method relations, captured source-byte
copies and SQL table impact. All fixtures/databases are temporary and embeddings
mocked. Production data is untouched.

Final executable reports store schema **25**, extraction format **58**. Global
format advanced for the new capture/Unicode semantics and concurrent Rust work;
the key capability also has its own per-document and frozen-graph marker. Exact
executable, log, source and pinned-reference hashes are in `verification.json`.
The dirty shared checkout remained concurrently active: this qualifies the
checked build and isolated executable, not a frozen checkout reconstruction.

Translation catalogue/locale lookup, framework binding, additional formats,
named/dynamic PHP forms, shared-key symbol enumeration, remaining CLI/HTTP
surfaces, crash/platform/scale and representative precision/recall remain open.
Completeness flags cover the documented syntax subset, never runtime absence.
