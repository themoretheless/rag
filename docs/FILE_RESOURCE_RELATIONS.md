# Captured HTML/CSS file resource candidates

`impact(impact_file=...)` in references/default or all mode now traverses separate
`file_resource_candidate` edges. The files view of `dead_code` protects targets
with known incoming resource candidates. No calls, routine reachability, CSS
cascade or browser execution are inferred. A stylesheet-to-stylesheet import can
therefore connect through a page that links the importing stylesheet.

The collector uses tree-sitter nodes, not whole-document string matches:

- HTML literal href/src/data attributes on link/a/script/iframe/img/source/video/
  audio/embed/input/object, form action, button/input formaction, and video poster. Duplicate attributes are skipped.
  Entities are decoded. Comments and script-body text do not create tags.
- CSS quoted `@import` and literal `url(...)` import arguments, plus literal URL
  values in declarations. Condition/namespace URL arguments do not load files
  here. Inline style-element CSS uses the containing document path and full
  original source offsets. Escaped/dynamic CSS values are unsupported.

The exact relative path is resolved against the captured source's parent.
HTML entities are decoded; URL queries/fragments are removed only for path
matching, with the original decoded URL retained in evidence. Candidates are
fresh captured parsed documents or published non-code source identities, with
equal project identity. Resource source extraction still requires a fresh parse.
See [target identities](RESOURCE_TARGET_IDENTITIES.md) for the non-code contract.
No extension/index-file guessing, network fetch or source execution occurs.
Multiple captured matches remain explicitly ambiguous. Evidence includes source
revision, content hash, line/byte offset and syntax kind; bodies are omitted.

`<base href>` anywhere in a document leaves all its attribute and inline-CSS URLs
unresolved, including those before the base tag. Root-relative/protocol-relative
URLs need a deployment root/origin and remain unresolved. Schemes, percent or
backslash path encodings, path template markers/whitespace/control characters, empty/
fragment-only paths and directory URLs are not guessed. Facts are capped at
50,000 per document and URLs at 4,096 bytes; the graph is not exhaustive. The
limits do not establish a total parse/query IO, time or memory budget.

These are conservative file-layout candidates, not proof of deployed URL routing
or runtime fetching. Unregistered assets and binary byte fidelity, generated/framework links, preprocessors and
modern CSS syntax outside the installed grammar remain open. Parse-error files
are excluded; in particular CSS @import supports(...) may be unparseable by the
current grammar. A missing edge or possible dead-file candidate never proves a
file safe to delete. Runtime resource use and scale/precision are unqualified.

Extraction format 35 stores resource specs; schema 25 is unchanged. Sync replaces
facts. Frozen graphs store `file_resources`, and diff reports this relation
family separately. If either historical graph lacks that array,
`file_resource_relations_available=false`; no resource changes are fabricated.

Qualification: [file-resources-2026-10-25](qualification/file-resources-2026-10-25/README.md).

Format 36 additionally captures non-code source attachment identities with independent freshness and historical availability. JSON and XML/SVG text targets now participate without being counted as parser-supported code.

The format 38 qualification adds [responsive image candidates](RESPONSIVE_RESOURCE_RELATIONS.md) for srcset/imagesrcset and CSS image-set, with source anchors, bounded descriptors, explicit selection limits and independently guarded historical comparison.

Format 40 adds [style-attribute resource candidates](INLINE_STYLE_RESOURCE_RELATIONS.md), including entity-aware source anchors, declaration-list recovery, element-base guards and independent historical availability.
