# Ruby code intelligence

Stored `.rb` sources now use tree-sitter-ruby 0.23.1. This continues the Ruby
language gap in the pinned RAGRAF profile (`extract/generic.py`: module/class,
method/singleton_method, call and superclass/import syntax). Ruby is no longer
only a searchable text format. No Ruby VM, gem loader or source code is executed.

The extractor records source-bound modules, classes, ordinary/singleton methods,
endless methods and supported eigenclass-self methods. Instance and singleton
method names are distinct (`Owner#method`, `Owner.method`); source spans support
explicit exact body reads. Definition signatures are bounded. Calls and constant
uses retain document/path/revision/hash/line/byte evidence; call expressions omit
receiver argument bodies. Bare local/parameter reads remain value references.

Captured-owner lookup covers implicit/literal self, captured constant receivers,
literal `.new` receivers and uniquely assigned straight-line constructor locals.
It separates parameters/block shadows and tuple/loop/rescue/pattern bindings and rejects reassigned/conditional or
unknown local receiver typing. Captured constructor overrides, including known
ancestor overrides, prevent invented instance typing. Top-level and nested
namespace contexts are kept separate; absolute constants do not fall through a
relative namespace. Qualified module declarations retain their actual lexical
nesting shape rather than assuming all textual prefixes are lexical parents.

Literal receiver-less `require_relative` creates a separate file import candidate
and extends the captured same-project Ruby scope, including transitive imports.
Exact normalized `.rb` source paths are used; requested files need no symbols.
`require`/`load` search paths, interpolation, escapes and external gems remain
unresolved. The import is syntactic evidence, not proof of Kernel identity, load
order or actual execution. Binding scope is bounded to 4,096 documents and 16,384
require steps. Incomplete traversal cannot produce successful method bindings.

Explicit single-superclass declarations have separately qualified inheritance
edges and method candidates. Captured mixin/alias/eval/method-mutation forms make
affected owner lookup unverified. Reopened duplicate methods remain ambiguous;
no runtime last-definition order is guessed. Unknown receivers, implicit external
Object/Kernel ancestors, dynamic definitions, arbitrary returns/fields/operators,
visibility/arity/refinement semantics and full Ruby dispatch remain unqualified.
Superclass edges never claim a complete Ruby method-resolution hierarchy.

This is informed by the official [method](https://docs.ruby-lang.org/en/master/syntax/methods_rdoc.html)
and [module/class](https://docs.ruby-lang.org/en/master/syntax/modules_and_classes_rdoc.html)
documentation inspected 2026-10-26. It is not Ruby VM/compiler conformance. RSpec,
Rails, Rake and other framework entrypoints are not inferred. `test_`/top-level
`main` are convention candidates. Instance `initialize` is a constructor-hook
candidate so implicit construction does not make it a deletion candidate; no
synthetic constructor call is invented. Setter/operator dispatch is unverified. Structural slices/dead-code checks keep runtime
uncertainty explicit; candidates are never deletion permission.

Extraction format 42 adds persisted Ruby facts and `ruby_syntax_format` in frozen
graphs; format 44 includes the local-binding and constructor-hook corrections.
Schema 25 is unchanged. Symbol/reference/body/caller/callee/file impact,
source sync, immutable history and binding-cache/full-recompute equivalence use
the existing stores. `ruby_relations_available` guards Ruby import/superclass
comparison against legacy absent coverage. `analysis_format_changed` still warns
that symbol/call changes across extractor versions may reflect analysis support.
Index status advertises partial static Ruby grammar support, not runtime proof.

Representative precision/recall, indexing/lookup scale, all language forms,
frameworks, load-path/platform behavior and complete RAGRAF parity remain open.
Qualification: [ruby-index-2026-10-26](qualification/ruby-index-2026-10-26/README.md).
