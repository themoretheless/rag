# Responsive image resource candidates

HTML img/source `srcset`, link `imagesrcset` and CSS `image-set()`/
`-webkit-image-set()` now participate in file-resource impact and dead-file
protection. Targets use the existing captured parsed/non-code source identities,
project and freshness guards. No browser, image decoder or source selection runs.

HTML collection uses the attribute's decoded value and a tokenizer shaped by the
[HTML Standard srcset algorithm](https://html.spec.whatwg.org/multipage/images.html#parsing-a-srcset-attribute), inspected 2026-10-26. URL tokens retain internal
commas, including data URLs. Trailing separator commas and descriptor parentheses
are handled without a naïve split on every comma. Supported descriptor validation
covers width, finite nonnegative density and width-associated future height;
unknown, conflicting, negative or unrepresentable descriptors are omitted with a
persisted limitation. Zero density is retained. Duplicate attributes are skipped.
No sizes/media/viewport/source ordering or density selection is inferred.

CSS collection reads AST argument groups. A literal string or literal `url()`
in the image position produces a candidate. Literal resolutions and optional
plain `type()` hints are retained separately. A known literal URL with a math/
variable resolution expression remains a candidate labelled
`resolution_expression_unverified`. Generated/dynamic image expressions and
unsupported option shapes have explicit limitations. Generic URL extraction is
suppressed within image-set handling so rejected options, nested sets, condition
expressions and MIME strings do not leak ordinary file edges. Type support,
property validity, duplicate-resolution selection and math evaluation are not
proven. This is structural evidence informed by the
[CSS Images 4 working draft](https://drafts.csswg.org/css-images-4/#image-set-notation),
not CSS or browser conformance certification.

Each responsive edge has `choice` evidence: syntax, bounded descriptor spelling,
full-descriptor hash, optional type hint, validation qualification, source anchor
and end byte, with `selection_proven=false`. HTML anchors identify the original
whole attribute value, not guessed per-URL offsets after entity decoding. CSS
anchors identify the original image literal, including inline style-element
base offsets. Descriptor text is bounded to 128 characters per stored descriptor;
truncation is explicit and a full hash retains change detection. Existing per-
document resource-fact/URL limits still apply, without a total query IO/RSS claim.

All existing base/root/path-encoding guards remain. In particular `<base href>`
affects both srcset and inline CSS; root-relative URLs need deployment metadata.
Data/scheme URLs are not guessed into local files. Candidate membership does not
prove actual loading, device/type support or deletion safety. JSX/generated
bindings, runtime picture/preload semantics, escaped CSS strings, generated
images, full CSS validity/variable substitution and representative precision/
recall/scale remain unqualified.

The qualified runtime uses extraction format 38 with choice facts and a frozen responsive capability marker;
store schema 25 is unchanged. Sync replaces facts. Frozen diff distinguishes
descriptor/type changes, while physical line/byte relocation alone does not
change relation keys. Descriptor spelling/expression hashes are compared without
claiming CSS semantic equivalence. `responsive_resource_relations_available`
requires support on both sides. Cross-capability comparisons also expose
`css_url_classification_comparable=false` and omit affected generic CSS URL
comparisons; they do not invent removals for the old image-set URL classification.
Other independently comparable resource relations remain available.

Qualification: [responsive-resources-2026-10-26](qualification/responsive-resources-2026-10-26/README.md).
