# HTML style-attribute resource candidates

Literal HTML `style` attributes now contribute file-resource candidates for CSS
`url()` and supported `image-set()` options. They participate in exact file impact,
transitive incoming traversal, dead-file protection, source-sync replacement and
immutable relation diffs. No CSS cascade, browser execution or image loading is
performed. Existing captured-target/project/freshness rules remain applicable.

The input is an entity-decoded declaration list, following the distinction in
[CSS Style Attributes](https://www.w3.org/TR/css-style-attr/#syntax), inspected
2026-10-26. Declaration boundaries use cssparser 0.36, already present transitively
and now an explicit dependency. Qualified rules and at-rules are rejected in this
context. Each accepted declaration is passed independently to the existing CSS
AST resource collectors; failures do not turn the containing HTML into a parse
error or remove independently valid neighboring declarations. Closing braces
are handled by declaration-list recovery rather than terminating a synthetic
whole-attribute stylesheet.

Only resource facts are merged back. Synthetic selectors, class definitions,
code symbols/references, paths and parser offsets are not exposed. CSS comments,
ordinary string contents and nested rule bodies do not create resource edges.
Duplicate style attributes, binding attributes such as `:style`, dynamic paths
and CSS forms outside the installed extractor grammar remain unqualified.

Each inline resource retains an `inline` evidence object:

- `source_anchor=html_style_attribute_value` and the original attribute end byte.
  The edge start byte/line identify that same original attribute, including when
  entity decoding changes string lengths or the page contains multibyte text.
- Canonical property name, bounded to 128 characters with explicit truncation and
  a hash over the full name. Ordinary names use ASCII lower case; custom-property
  names retain case. No property validity/cascade conclusion is implied.
- Declaration start/end coordinates in the entity-decoded attribute and its full
  text hash. These are explicitly not guessed per-URL source coordinates.
- The underlying CSS reference kind and any responsive choice qualification.
  Responsive end/anchor fields likewise refer to the original HTML attribute.

Whole-document `<base href>` and inherited element `xml:base` leave affected
relative targets unresolved. Element base handling is conservative and does not
claim namespace/deployment URI evaluation. Root/scheme/encoding guards remain.
Escaped CSS values, arbitrary property/variable semantics, framework/JSX styles,
computed URLs and full CSS/browser conformance remain unqualified.

Attributes exceeding 256 KiB are omitted with a persisted limitation. Nested CSS
component validation has a depth limit of 128; resource tree traversal has 256.
Existing per-document resource-fact/URL limits remain. These limits do not qualify
total parse/query IO, CPU time or memory; representative scale remains open.

Extraction format 40 introduces inline evidence and a frozen capability marker;
store schema 25 is unchanged. `inline_style_relations_available` requires support
on both historical sides. Unavailable legacy inline families are excluded without
invented additions/removals. Available keys include property identity and
responsive descriptors, while excluding anchor coordinates/whole-attribute hashes
so line relocation or equivalent entity spelling does not create link churn.
This is dependency/descriptor spelling comparison, not CSS semantic equivalence.

Qualification: [inline-style-resources-2026-10-26](qualification/inline-style-resources-2026-10-26/README.md).
