# Rust raw pointer extension-trait qualification

Bounded milestone passed. The complete code-intelligence plan remains open.

## Controlled quality results

| Repository | Before correct | After correct | Labels | False targets after |
|---|---:|---:|---:|---:|
| itoa | 9 | 9 | 9 | 0 |
| ryu | 93 | 93 | 95 | 0 |
| memchr | 313 | 357 | 466 | 0 |
| Combined | 415 | 459 | 570 | 0 |

All 44 recovered sites are memchr direct calls. No previously correct site is
lost. Combined direct calls improve 369/486 to 413/486; imports remain 39/39,
value/other references 7/7 and macro-token references 0/38.

Original pinned sources, captured Cargo/config inputs and independently derived
rust-analyzer SCIP/CST oracle/labels are unchanged. Baseline hashes point to the
preceding tuple-field milestone and were byte-checked against the working
baseline. The summarizer recomputes metrics, verifies original label/source
identity and native/actual-MCP parity, and enforces one frozen server/helper pair
per phase. Scored cases and transitions retain original ranges and before/after
observations. The 570-label denominator is preserved.

This is source-definition correspondence on a small library corpus with no
external-negative labels, not universal precision or observed runtime dispatch.
Original RA error logs, excluded ambiguous/generated anchors and configuration
limits remain in the original independent corpus qualification. No denominator
or source rewrite was used to improve the score.

## Implementation and negative boundaries

Format 68 preserves raw-pointer const/mut identity and primitive pointee names.
Explicit cast targets participate in typing, captured extension-trait impl
patterns match pointee identity and existing Self substitution supports return
chains. Pointer methods do not auto-dereference to pointee methods. Unknown
pointees cannot bind a generic impl. Known str/slice/dyn and unmodeled pointees
cannot satisfy implicit Sized blanket parameters just by pointer shape.

Pointer/primitive inherent impls are not accepted as local method owners. Their
extension traits must share a captured target with the impl. Dot calls matching
known inherent sysroot pointer names remain unresolved instead of borrowing an
extension-trait target. The reserved names were checked against the Rust 1.99.1
[pointer inventory](https://doc.rust-lang.org/std/primitive.pointer.html) on
2026-10-26; this is a bounded name guard, not runtime sysroot/compiler inference.

Four native regressions cover const/mut and pointee separation, cast targets,
no automatic dereference, exact primitive impl matching, implicit Sized guards
and sysroot-name precedence. Deliberately invalid negative source snippets are
extraction tests, not claimed as compiler-valid programs.

The compiler-valid actual MCP fixture checks five exact targets: const and mut
mark calls, a mut-to-const cast, and both links in pointer.again().mark(). Warm
indexing reuses the revision. Cargo executes one passing test distinguishing
values [11,22,11,11]. No pointer dereference or unsafe program execution is needed.

## Executables and checks

Frozen server SHA-256:
172bde2c302ede19df0247c6b907d1710c9dc6013f1646a6e71cb23a35cbb91d.
Native helper SHA-256:
cca600b30661f9f0bdeece8e2d92d7ce3518d17a92219ff64dc895b9a074d41a.
Index format 68. All final runtime/corpus results use this pair; binary mtimes
postdate the final semantic source edits. Focused Rust: 80 passed. Workspace:
1522 passed, zero failed, one ignored. Strict Clippy passed, three Python
adapter/metric tests passed and the smoke runner byte-compiles. Logs and
checks.json retain the checks.

The shared dirty source checkout is not frozen; unrelated concurrent work is
not attributed here. Runtime binaries were frozen before qualification.
Production executable unchanged; qualification stores are disposable/exclusive.
No release-speed/RSS/competitor-superiority conclusion follows from these runs.

## Remaining scope

111 original labels remain unresolved: 109 memchr and two ryu. Memchr has 53
receiver-typing observations, 38 macro observations, 16 missing extracted
observations, one module-path and one shadow/external observation. Ryu has two
missing observations. Complete Sized/unsized/generic inference, pointer arithmetic
returns, raw borrows/coercions, sysroot method resolution, compiler/build semantics,
representative external-negative quality and equal-work release competitor
performance remain unqualified. Full goal completion remains unproven.
