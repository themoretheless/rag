//! Bounded declared class chains and trait providers; no runtime subtype dispatch.
use super::*;
type Failure = &'static str;
const MAX_CHAIN: usize = 64;

pub(super) struct Hierarchy<'a> {
    pub(super) definitions: HashMap<&'a str, &'a Definition>,
    pub(super) scopes: HashMap<&'a str, &'a Scope>,
    classes: HashMap<String, Vec<&'a Definition>>,
    methods: HashMap<(&'a str, String), Vec<&'a Definition>>,
    pub(super) context_cache: std::cell::RefCell<HashMap<String, super::contexts::Consumers>>,
}
impl<'a> Hierarchy<'a> {
    pub(super) fn new(docs: &'a HashMap<String, IndexedDocument>, doc: &IndexedDocument) -> Self {
        let mut result = Self {
            definitions: HashMap::new(),
            scopes: HashMap::new(),
            classes: HashMap::new(),
            methods: HashMap::new(),
            context_cache: std::cell::RefCell::new(HashMap::new()),
        };
        for facts in docs
            .values()
            .filter(|d| d.project == doc.project && !d.parse_errors)
            .filter_map(|d| d.php_names.as_ref())
            .filter(|f| f.format == 6 && f.complete)
        {
            for d in &facts.definitions {
                let Some(scope) = facts.scopes.get(d.scope).filter(|s| s.complete) else {
                    continue;
                };
                result.definitions.insert(&d.id, d);
                result.scopes.insert(&d.id, scope);
                if d.kind == "class" {
                    result
                        .classes
                        .entry(d.fqn.to_ascii_lowercase())
                        .or_default()
                        .push(d);
                } else if d.kind == "method" {
                    if let Some(owner) = &d.owner {
                        result
                            .methods
                            .entry((
                                owner.as_str(),
                                d.fqn.rsplit("::").next().unwrap_or("").to_ascii_lowercase(),
                            ))
                            .or_default()
                            .push(d);
                    }
                }
            }
        }
        result
    }
    pub(super) fn named(&self, scope: &Scope, name: &str) -> Vec<&'a Definition> {
        let (names, _) = super::resolve::targets(scope, name, "class");
        names
            .iter()
            .flat_map(|n| {
                self.classes
                    .get(&n.to_ascii_lowercase())
                    .into_iter()
                    .flatten()
                    .copied()
            })
            .collect()
    }
    pub(super) fn class(&self, id: &str) -> std::result::Result<&'a Definition, Failure> {
        let d = self
            .definitions
            .get(id)
            .copied()
            .ok_or("PHP_CLASS_TARGET_NOT_CAPTURED")?;
        if d.conditional {
            return Err("PHP_CONDITIONAL_DECLARATION_UNVERIFIED");
        }
        if self
            .classes
            .get(&d.fqn.to_ascii_lowercase())
            .is_none_or(|v| v.len() != 1)
        {
            return Err("PHP_CLASS_IDENTITY_AMBIGUOUS");
        }
        if !matches!(
            d.declaration_kind.as_str(),
            "class_declaration" | "enum_declaration"
        ) {
            return Err("PHP_NONCONCRETE_OWNER_UNVERIFIED");
        }
        Ok(d)
    }
    fn parent(&self, d: &Definition) -> std::result::Result<Option<&'a Definition>, Failure> {
        let bases: Vec<_> = d.bases.iter().filter(|b| b.relation == "extends").collect();
        if bases.is_empty() {
            return Ok(None);
        }
        if bases.len() != 1 || d.declaration_kind != "class_declaration" {
            return Err("PHP_CLASS_BASE_SHAPE_UNVERIFIED");
        }
        let scope = self
            .scopes
            .get(d.id.as_str())
            .ok_or("PHP_SCOPE_IMPORT_TABLE_INCOMPLETE")?;
        let candidates = self.named(scope, &bases[0].expression);
        if candidates.is_empty() {
            return Err("PHP_BASE_TARGET_NOT_CAPTURED");
        }
        if candidates.len() != 1 {
            return Err("PHP_BASE_IDENTITY_AMBIGUOUS");
        }
        let parent = self.class(&candidates[0].id)?;
        if parent.declaration_kind != "class_declaration" || parent.is_final {
            return Err("PHP_INVALID_BASE_KIND_OR_FINAL");
        }
        Ok(Some(parent))
    }
    /// Complete captured ancestry, even if a method is found before the last base.
    fn chain(&self, id: &str) -> std::result::Result<Vec<&'a Definition>, Failure> {
        let mut current = Some(self.class(id)?);
        let mut seen = HashSet::new();
        let mut chain = Vec::new();
        while let Some(d) = current {
            if !seen.insert(&d.id) {
                return Err("PHP_INHERITANCE_CYCLE");
            }
            if chain.len() == MAX_CHAIN {
                return Err("PHP_INHERITANCE_DEPTH_LIMIT");
            }
            chain.push(d);
            current = self.parent(d)?;
        }
        Ok(chain)
    }
    fn related(&self, a: &str, b: &str) -> bool {
        a == b
            || self.chain(a).is_ok_and(|c| c.iter().any(|d| d.id == b))
            || self.chain(b).is_ok_and(|c| c.iter().any(|d| d.id == a))
    }
    pub(super) fn declared(
        &self,
        owner: &str,
        name: &str,
    ) -> std::result::Result<Option<&'a Definition>, Failure> {
        let Some(found) = self.methods.get(&(owner, name.to_ascii_lowercase())) else {
            return Ok(None);
        };
        if found.len() != 1 {
            return Err("PHP_METHOD_DECLARATION_AMBIGUOUS");
        }
        if found[0].conditional {
            return Err("PHP_CONDITIONAL_DECLARATION_UNVERIFIED");
        }
        Ok(Some(found[0]))
    }
    fn lookup(
        &self,
        owner: &str,
        name: &str,
    ) -> std::result::Result<(super::traits::Effective<'a>, bool), Failure> {
        let class = self.class(owner)?;
        if let Some(d) = self.declared(owner, name)? {
            return Ok((super::traits::Effective::declared(d), false));
        }
        if let Some(d) = super::traits::method(self, class, name)? {
            return Ok((d, false));
        }
        let chain = self.chain(owner)?;
        for base in chain.iter().skip(1) {
            let found = if let Some(d) = self.declared(&base.id, name)? {
                Some(super::traits::Effective::declared(d))
            } else {
                super::traits::method(self, base, name)?
            };
            if let Some(d) = found {
                if d.visibility == "private" {
                    return Err("PHP_PRIVATE_BASE_METHOD_NOT_INHERITED");
                }
                return Ok((d, true));
            }
        }
        Err("PHP_METHOD_TARGET_NOT_CAPTURED")
    }
    pub(super) fn method(&self, u: &Use, scope: &Scope) -> (Vec<String>, Failure, Option<String>) {
        let result = (|| {
            if u.target.eq_ignore_ascii_case("static") {
                return Err("PHP_LATE_STATIC_BINDING_UNVERIFIED");
            }
            let own = u.target.eq_ignore_ascii_case("self") || u.target == "$this";
            let parent_call = u.target.eq_ignore_ascii_case("parent");
            let owner = if own || parent_call {
                let owner = u.owner.as_deref().ok_or("PHP_THIS_CONTEXT_UNAVAILABLE")?;
                let class = self.class(owner)?;
                if u.target == "$this" && !u.instance {
                    return Err("PHP_THIS_CONTEXT_UNAVAILABLE");
                }
                if parent_call {
                    // Validate the whole chain, including cycles above the immediate base.
                    let chain = self.chain(owner)?;
                    chain
                        .get(1)
                        .copied()
                        .ok_or("PHP_PARENT_CLASS_NOT_CAPTURED")?
                } else {
                    class
                }
            } else {
                let owners = self.named(scope, &u.target);
                if owners.is_empty() {
                    return Err("PHP_CLASS_TARGET_NOT_CAPTURED");
                }
                if owners.len() != 1 {
                    return Err("PHP_CLASS_IDENTITY_AMBIGUOUS");
                }
                self.class(&owners[0].id)?
            };
            let (method, inherited) = self.lookup(&owner.id, &u.reference.name)?;
            if !method.definition.has_body {
                return Err("PHP_ABSTRACT_METHOD_DISPATCH_UNVERIFIED");
            }
            let defining = method.host;
            let trait_method = method.from_trait;
            if !method.definition.is_static
                && !(u.instance
                    && u.owner.as_deref().is_some_and(|context| {
                        // Scoped non-static calls require the current instance to be of the target class.
                        context == owner.id
                            || self
                                .chain(context)
                                .is_ok_and(|c| c.iter().any(|d| d.id == owner.id))
                    }))
            {
                return Err("PHP_NONSTATIC_METHOD_WITHOUT_COMPATIBLE_INSTANCE");
            }
            let visible = match method.visibility.as_str() {
                "public" => true,
                "private" => u.owner.as_deref() == Some(defining),
                "protected" => u
                    .owner
                    .as_deref()
                    .is_some_and(|context| self.related(context, defining)),
                _ => false,
            };
            if !visible {
                return Err("PHP_METHOD_VISIBILITY_UNVERIFIED");
            }
            Ok((
                vec![method.definition.id.clone()],
                if trait_method {
                    "PHP_TRAIT_METHOD_CANDIDATE_SIGNATURE_AND_RUNTIME_UNVERIFIED"
                } else if inherited || parent_call {
                    "PHP_INHERITED_METHOD_CANDIDATE_RUNTIME_DISPATCH_UNVERIFIED"
                } else {
                    "PHP_DECLARED_METHOD_CANDIDATE_DISPATCH_VISIBILITY_AND_BASES_PARTIAL"
                },
                trait_method.then(|| method.host.to_string()),
            ))
        })();
        result.unwrap_or_else(|reason| (Vec::new(), reason, None))
    }
}

pub(super) fn edges(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<super::super::inheritance::InheritanceEdge> {
    let Some(facts) = &doc.php_names else {
        return vec![];
    };
    let hierarchy = Hierarchy::new(docs, doc);
    let mut result = Vec::new();
    for d in facts.definitions.iter().filter(|d| d.kind == "class") {
        for group in &d.trait_uses {
            for expression in &group.names {
                let mut candidates: Vec<_> =
                    if facts.complete && group.complete && !doc.parse_errors {
                        facts
                            .scopes
                            .get(d.scope)
                            .filter(|s| s.complete)
                            .into_iter()
                            .flat_map(|scope| hierarchy.named(scope, expression))
                            .filter(|t| t.declaration_kind == "trait_declaration" && !t.conditional)
                            .map(|t| t.id.clone())
                            .collect()
                    } else {
                        Vec::new()
                    };
                candidates.sort();
                candidates.dedup();
                result.push(super::super::inheritance::InheritanceEdge {
                    derived: d.id.clone(),
                    resolution: match candidates.len() {
                        0 => "unresolved",
                        1 => "resolved_static",
                        _ => "ambiguous",
                    }
                    .into(),
                    candidates,
                    base_expression: expression.clone(),
                    path: doc.path.clone(),
                    line: group.line,
                    relation: "php_trait_use_candidate".into(),
                    hierarchy_status: "php_trait_composition_signature_and_runtime_unverified"
                        .into(),
                    provenance: "derived",
                });
            }
        }
        for base in &d.bases {
            let mut candidates: Vec<_> = if facts.complete && !doc.parse_errors {
                facts
                    .scopes
                    .get(d.scope)
                    .filter(|s| s.complete)
                    .into_iter()
                    .flat_map(|s| hierarchy.named(s, &base.expression))
                    .filter(|parent| {
                        !parent.conditional
                            && if d.declaration_kind == "interface_declaration"
                                || base.relation == "implements"
                            {
                                parent.declaration_kind == "interface_declaration"
                            } else {
                                parent.declaration_kind == "class_declaration"
                            }
                    })
                    .map(|d| d.id.clone())
                    .collect()
            } else {
                vec![]
            };
            candidates.sort();
            candidates.dedup();
            let complete = hierarchy.chain(&d.id).is_ok();
            result.push(super::super::inheritance::InheritanceEdge {
                derived: d.id.clone(),
                resolution: match candidates.len() {
                    0 => "unresolved",
                    1 => "resolved_static",
                    _ => "ambiguous",
                }
                .into(),
                candidates,
                base_expression: base.expression.clone(),
                path: doc.path.clone(),
                line: base.line,
                relation: base.relation.clone(),
                hierarchy_status: if complete {
                    "php_captured_class_chain_runtime_and_composition_unverified"
                } else {
                    "incomplete_or_invalid"
                }
                .into(),
                provenance: "derived",
            });
        }
    }
    result
}

pub(super) fn overrides(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<super::super::inheritance::OverrideEdge> {
    let Some(facts) = &doc.php_names else {
        return vec![];
    };
    if doc.parse_errors || !facts.complete {
        return vec![];
    }
    let hierarchy = Hierarchy::new(docs, doc);
    let mut result = Vec::new();
    for method in facts
        .definitions
        .iter()
        .filter(|d| d.kind == "method" && d.visibility != "private")
    {
        let Some(owner) = method.owner.as_deref() else {
            continue;
        };
        let Ok(chain) = hierarchy.chain(owner) else {
            continue;
        };
        if chain[0].uses_traits {
            continue;
        }
        let name = method.fqn.rsplit("::").next().unwrap_or("");
        if name.eq_ignore_ascii_case("__construct") {
            continue;
        }
        for base in chain.iter().skip(1) {
            match hierarchy.declared(&base.id, name) {
                Ok(Some(parent)) => {
                    if parent.visibility != "private"
                        && !parent.is_final
                        && parent.is_static == method.is_static
                    {
                        if let Some(symbol) = doc.symbols.iter().find(|s| s.id == method.id) {
                            result.push(super::super::inheritance::OverrideEdge {
                                derived: method.id.clone(),
                                owner: owner.into(),
                                candidates: vec![parent.id.clone()],
                                document_id: symbol.document_id.clone(),
                                path: doc.path.clone(),
                                line: symbol.start_line,
                                revision: doc.revision,
                                content_hash: doc.hash.clone(),
                                resolution: "resolved_static",
                                resolution_basis: "php_declared_method_name_in_captured_base_chain",
                                semantic_override_proven: false,
                            });
                        }
                    }
                    break;
                }
                Err(_) => break,
                Ok(None) => {
                    if base.uses_traits {
                        break;
                    }
                }
            }
        }
    }
    result
}
