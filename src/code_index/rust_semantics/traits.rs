//! Captured impl ownership, trait-bound declarations and generic substitutions.
//! A bound/dyn receiver links to trait declarations, never guessed concrete vtables.
use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    pub(super) type_parameters: BTreeMap<String, Vec<String>>,
    implementations: Vec<Implementation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Implementation {
    owner: Ty,
    trait_type: Option<Ty>,
    parameters: Vec<String>,
    bounds: BTreeMap<String, Vec<Ty>>,
    methods: Vec<String>,
    associated: BTreeMap<String, Ty>,
    start: usize,
    supported: bool,
    #[serde(default)]
    conditional: bool,
    #[serde(default)]
    negative: bool,
}
fn parameters(node: Node, source: &str) -> Vec<String> {
    node.child_by_field_name("type_parameters")
        .map(|p| {
            children(p)
                .into_iter()
                .filter(|n| n.kind() == "type_parameter")
                .filter_map(|n| n.child_by_field_name("name"))
                .map(|n| text(n, source).to_string())
                .collect()
        })
        .unwrap_or_default()
}
pub(super) fn bound_types(node: Node, source: &str) -> Vec<Ty> {
    if matches!(node.kind(), "trait_bounds" | "bounded_type") {
        children(node)
            .into_iter()
            .filter(|n| n.kind() != "lifetime")
            .map(|n| ty(n, source, 0))
            .collect()
    } else {
        vec![ty(node, source, 0)]
    }
}
pub(super) fn bounds(node: Node, source: &str) -> BTreeMap<String, Vec<Ty>> {
    let mut result: BTreeMap<String, Vec<Ty>> = BTreeMap::new();
    let mut ancestor = Some(node);
    while let Some(parent) = ancestor {
        if let Some(p) = parent.child_by_field_name("type_parameters") {
            for parameter in children(p) {
                if let (Some(name), Some(b)) = (
                    parameter.child_by_field_name("name"),
                    parameter.child_by_field_name("bounds"),
                ) {
                    result
                        .entry(text(name, source).into())
                        .or_default()
                        .extend(bound_types(b, source));
                }
            }
        }
        for clause in children(parent)
            .into_iter()
            .filter(|n| n.kind() == "where_clause")
        {
            for predicate in children(clause) {
                if let (Some(left), Some(b)) = (
                    predicate.child_by_field_name("left"),
                    predicate.child_by_field_name("bounds"),
                ) {
                    result
                        .entry(text(left, source).into())
                        .or_default()
                        .extend(bound_types(b, source));
                }
            }
        }
        ancestor = parent.parent();
    }
    result
}
pub(super) fn collect(root: Node, source: &str, doc: &IndexedDocument) -> Facts {
    let mut facts = Facts::default();
    let mut stack = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > 128 {
            continue;
        }
        if matches!(node.kind(), "struct_item" | "enum_item" | "trait_item") {
            if let Some(s) = doc
                .symbols
                .iter()
                .find(|s| s.start_byte == node.start_byte())
            {
                facts
                    .type_parameters
                    .insert(s.id.clone(), parameters(node, source));
            }
        }
        if node.kind() == "impl_item" {
            if let (Some(owner), Some(body)) = (
                node.child_by_field_name("type"),
                node.child_by_field_name("body"),
            ) {
                let members = children(body);
                let methods = members
                    .iter()
                    .filter(|n| n.kind() == "function_item")
                    .filter_map(|n| {
                        doc.symbols
                            .iter()
                            .find(|s| s.start_byte == n.start_byte())
                            .map(|s| s.id.clone())
                    })
                    .collect();
                let associated = members
                    .iter()
                    .filter(|n| n.kind() == "type_item")
                    .filter_map(|n| {
                        Some((
                            text(n.child_by_field_name("name")?, source).into(),
                            ty(n.child_by_field_name("type")?, source, 0),
                        ))
                    })
                    .collect();
                let mut previous = node.prev_named_sibling();
                let mut supported = true;
                while let Some(p) = previous
                    .filter(|p| p.kind() == "attribute_item" || p.kind().ends_with("comment"))
                {
                    if p.kind() == "attribute_item" && text(p, source).contains("cfg") {
                        supported = false;
                    }
                    previous = p.prev_named_sibling();
                }
                let conditional = !supported;
                let negative = source[node.start_byte()..body.start_byte()].contains('!');
                supported &= !negative;
                facts.implementations.push(Implementation {
                    owner: ty(owner, source, 0),
                    trait_type: node.child_by_field_name("trait").map(|n| ty(n, source, 0)),
                    parameters: parameters(node, source),
                    bounds: bounds(node, source),
                    methods,
                    associated,
                    start: node.start_byte(),
                    supported,
                    conditional,
                    negative,
                });
            }
        }
        stack.extend(children(node).into_iter().map(|n| (n, depth + 1)));
    }
    facts
}
pub(super) fn configure(
    facts: &mut Facts,
    decision: &impl Fn(usize) -> Option<bool>,
    ids: &HashSet<String>,
) {
    facts.type_parameters.retain(|id, _| ids.contains(id));
    facts.implementations.retain_mut(|implementation| {
        if decision(implementation.start) != Some(true) {
            return false;
        }
        if implementation.conditional {
            implementation.supported = !implementation.negative;
        }
        implementation.methods.retain(|id| ids.contains(id));
        true
    });
}
pub(super) fn remap(facts: &mut Facts, ids: &HashMap<String, String>) {
    facts.type_parameters = std::mem::take(&mut facts.type_parameters)
        .into_iter()
        .filter_map(|(id, p)| ids.get(&id).map(|next| (next.clone(), p)))
        .collect();
    for implementation in &mut facts.implementations {
        implementation.methods = implementation
            .methods
            .iter()
            .filter_map(|id| ids.get(id).cloned())
            .collect();
    }
}

impl<'a> Engine<'a> {
    pub(super) fn associated_type(
        &mut self,
        actual: &ValueType,
        name: &str,
        trait_ids: &[String],
        depth: usize,
    ) -> ValueType {
        if depth > 24 || trait_ids.is_empty() || !self.step() {
            return ValueType::Unknown;
        }
        let project = trait_ids
            .first()
            .and_then(|id| self.symbol(id))
            .and_then(|s| self.docs.get(&s.document_id))
            .map(|d| d.project.clone());
        let mut found = vec![];
        for d in self
            .docs
            .values()
            .filter(|d| Some(d.project.clone()) == project)
        {
            for implementation in &d.rust_semantics.trait_data.implementations {
                let (Some(tr), Some(associated)) = (
                    &implementation.trait_type,
                    implementation.associated.get(name),
                ) else {
                    continue;
                };
                let Some(substitutions) = self.impl_match(d, implementation, actual, depth + 1)
                else {
                    continue;
                };
                if matches!(self.types(d,implementation.start,"",tr,&substitutions,depth+1),ValueType::Nominal(ref id,_) if trait_ids.contains(id))
                {
                    found.push(self.types(
                        d,
                        implementation.start,
                        "",
                        associated,
                        &substitutions,
                        depth + 1,
                    ));
                }
            }
        }
        if found.len() == 1 {
            found.pop().unwrap()
        } else {
            ValueType::Unknown
        }
    }
    pub(super) fn trait_ids(
        &mut self,
        doc: &IndexedDocument,
        at: usize,
        owner: &str,
        bounds: &[Ty],
        substitutions: &HashMap<String, ValueType>,
        depth: usize,
    ) -> Vec<String> {
        if depth > 24 || !self.step() {
            return vec![];
        }
        let mut ids = vec![];
        for bound in bounds {
            if let ValueType::Nominal(id, _) =
                self.types(doc, at, owner, bound, substitutions, depth + 1)
            {
                if self.symbol(&id).is_some_and(|s| s.kind == "trait_item") {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        ids.dedup();
        ids
    }
    fn pattern(
        &mut self,
        doc: &IndexedDocument,
        implementation: &Implementation,
        pattern: &Ty,
        actual: &ValueType,
        substitutions: &mut HashMap<String, ValueType>,
        depth: usize,
    ) -> bool {
        if depth > 24 || !self.step() {
            return false;
        }
        match (pattern, actual) {
            (Ty::Named(name, args), _)
                if args.is_empty() && implementation.parameters.contains(name) =>
            {
                if matches!(actual, ValueType::Unknown) {
                    return false;
                }
                if let Some(prior) = substitutions.get(name) {
                    prior == actual
                } else {
                    substitutions.insert(name.clone(), actual.clone());
                    true
                }
            }
            (Ty::Primitive(p), ValueType::Primitive(a)) => p == a,
            (Ty::Pointer(pm, p), ValueType::Pointer(am, a)) if pm == am => {
                // A blanket impl<T> has an implicit Sized bound. Unsized or
                // unmodeled pointees must not satisfy it by shape alone.
                if matches!(p.as_ref(), Ty::Named(name, args) if args.is_empty() && implementation.parameters.contains(name))
                    && (matches!(a.as_ref(), ValueType::Unknown | ValueType::Traits(_) | ValueType::Array(_))
                        || matches!(a.as_ref(), ValueType::Primitive(name) if name == "str"))
                {
                    return false;
                }
                self.pattern(doc, implementation, p, a, substitutions, depth + 1)
            }
            (Ty::Reference(p), ValueType::Reference(a)) | (Ty::Array(p), ValueType::Array(a)) => {
                self.pattern(doc, implementation, p, a, substitutions, depth + 1)
            }
            (Ty::Named(_, patterns), ValueType::Nominal(id, arguments)) => {
                let unknowns = implementation
                    .parameters
                    .iter()
                    .map(|p| (p.clone(), ValueType::Unknown))
                    .collect();
                let resolved =
                    self.types(doc, implementation.start, "", pattern, &unknowns, depth + 1);
                let ValueType::Nominal(candidate, _) = resolved else {
                    return false;
                };
                candidate == *id
                    && patterns.len() == arguments.len()
                    && patterns.iter().zip(arguments).all(|(p, a)| {
                        self.pattern(doc, implementation, p, a, substitutions, depth + 1)
                    })
            }
            _ => false,
        }
    }
    fn impl_match(
        &mut self,
        doc: &IndexedDocument,
        implementation: &Implementation,
        actual: &ValueType,
        depth: usize,
    ) -> Option<HashMap<String, ValueType>> {
        if !implementation.supported || depth > 16 || !self.step() {
            return None;
        }
        let mut substitutions = HashMap::new();
        if !self.pattern(
            doc,
            implementation,
            &implementation.owner,
            actual,
            &mut substitutions,
            depth + 1,
        ) {
            return None;
        }
        if let ValueType::Nominal(id, _) = actual {
            if let Some(owner) = self.symbol(id).and_then(|s| self.docs.get(&s.document_id)) {
                if !rust_cargo::share_target(self.docs, doc, owner) {
                    let t = implementation.trait_type.as_ref()?;
                    let ValueType::Nominal(trait_id, _) =
                        self.types(doc, implementation.start, "", t, &substitutions, depth + 1)
                    else {
                        return None;
                    };
                    let declaration = self
                        .symbol(&trait_id)
                        .and_then(|s| self.docs.get(&s.document_id))?;
                    if !rust_cargo::share_target(self.docs, doc, declaration) {
                        return None;
                    }
                }
            }
        }
        for (parameter, bounds) in &implementation.bounds {
            let value = substitutions.get(parameter)?;
            let traits = self.trait_ids(
                doc,
                implementation.start,
                "",
                bounds,
                &substitutions,
                depth + 1,
            );
            if traits.len() != bounds.len() {
                return None;
            }
            for trait_id in traits {
                if !self.implements(value, &trait_id, depth + 1) {
                    return None;
                }
            }
        }
        substitutions.insert("Self".into(), actual.clone());
        Some(substitutions)
    }
    fn implements(&mut self, actual: &ValueType, trait_id: &str, depth: usize) -> bool {
        if depth > 16 || !self.step() {
            return false;
        }
        if let ValueType::Traits(ids) = actual {
            return ids.iter().any(|id| id == trait_id);
        }
        let project = self
            .symbol(trait_id)
            .and_then(|s| self.docs.get(&s.document_id))
            .map(|d| d.project.clone());
        for d in self
            .docs
            .values()
            .filter(|d| Some(d.project.clone()) == project)
        {
            for implementation in &d.rust_semantics.trait_data.implementations {
                let Some(t) = &implementation.trait_type else {
                    continue;
                };
                let Some(substitutions) = self.impl_match(d, implementation, actual, depth + 1)
                else {
                    continue;
                };
                if matches!(self.types(d,implementation.start,"",t,&substitutions,depth+1),ValueType::Nominal(ref id,_) if id==trait_id)
                {
                    return true;
                }
            }
        }
        false
    }
    fn declared_methods(
        &self,
        trait_id: &str,
        name: &str,
        receiver: bool,
        default_only: bool,
    ) -> Vec<&'a Symbol> {
        let Some(declaration) = self.symbol(trait_id) else {
            return vec![];
        };
        let qualified = format!("{}::{name}", declaration.qualified_name);
        self.symbols
            .iter()
            .filter(|s| {
                s.document_id == declaration.document_id
                    && s.qualified_name == qualified
                    && (!default_only || s.kind == "function_item")
                    && self
                        .docs
                        .get(&s.document_id)
                        .and_then(|d| d.rust_semantics.functions.get(&s.id))
                        .is_some_and(|f| !receiver || f.receiver)
            })
            .collect()
    }
    pub(super) fn trait_methods(
        &mut self,
        actual: &ValueType,
        name: &str,
        context: (&IndexedDocument, usize, &str),
        selected: Option<&str>,
        receiver: bool,
        depth: usize,
    ) -> Vec<&'a Symbol> {
        if depth > 24 || !self.step() {
            return vec![];
        }
        let (doc, at, owner) = context;
        if let ValueType::Traits(ids) = actual {
            return ids
                .iter()
                .filter(|id| selected.is_none_or(|s| s == id.as_str()))
                .flat_map(|id| self.declared_methods(id, name, receiver, false))
                .collect();
        }
        let mut inherent = vec![];
        let mut candidates = vec![];
        for d in self.docs.values().filter(|d| d.project == doc.project) {
            for implementation in &d.rust_semantics.trait_data.implementations {
                let Some(substitutions) = self.impl_match(d, implementation, actual, depth + 1)
                else {
                    continue;
                };
                let owner_document = if let ValueType::Nominal(id, _) = actual {
                    self.symbol(id).and_then(|s| self.docs.get(&s.document_id))
                } else {
                    None
                };
                if implementation.trait_type.is_none()
                    && owner_document
                        .is_some_and(|owner| !rust_cargo::share_target(self.docs, d, owner))
                {
                    continue;
                }
                if matches!(actual, ValueType::Pointer(..) | ValueType::Primitive(..))
                    && implementation.trait_type.is_none()
                {
                    continue;
                }
                let trait_id = if let Some(t) = &implementation.trait_type {
                    let ValueType::Nominal(id, _) =
                        self.types(d, implementation.start, "", t, &substitutions, depth + 1)
                    else {
                        continue;
                    };
                    if self.symbol(&id).is_none_or(|s| s.kind != "trait_item") {
                        continue;
                    }
                    let trait_document =
                        self.symbol(&id).and_then(|s| self.docs.get(&s.document_id));
                    if matches!(actual, ValueType::Pointer(..) | ValueType::Primitive(..))
                        && trait_document.is_none_or(|declaration| !rust_cargo::share_target(self.docs, d, declaration))
                    {
                        continue;
                    }
                    if owner_document
                        .is_some_and(|owner| !rust_cargo::share_target(self.docs, d, owner))
                        && trait_document.is_some_and(|declaration| {
                            !rust_cargo::share_target(self.docs, d, declaration)
                        })
                    {
                        continue;
                    }
                    if let Some(expected) = selected {
                        if expected != id {
                            continue;
                        }
                    } else {
                        let symbol = self.symbol(&id).unwrap();
                        if !self
                            .names(doc, at, owner, &symbol.name, false)
                            .iter()
                            .any(|s| s.id == id)
                            && !resolve::visible_import_targets(doc, at)
                                .iter()
                                .any(|target| {
                                    resolve::imported_type(
                                        self.symbols,
                                        self.docs,
                                        &doc.path,
                                        target,
                                        None,
                                    )
                                    .iter()
                                    .any(|s| s.id == id)
                                })
                        {
                            continue;
                        }
                    }
                    Some(id)
                } else {
                    if selected.is_some() {
                        continue;
                    }
                    None
                };
                let methods: Vec<_> = implementation
                    .methods
                    .iter()
                    .filter_map(|id| self.symbol(id))
                    .filter(|s| {
                        s.name == name
                            && d.rust_semantics
                                .functions
                                .get(&s.id)
                                .is_some_and(|f| !receiver || f.receiver)
                    })
                    .collect();
                match trait_id {
                    None => inherent.extend(methods),
                    Some(id) if methods.is_empty() => {
                        candidates.extend(self.declared_methods(&id, name, receiver, true))
                    }
                    Some(_) => candidates.extend(methods),
                }
            }
        }
        let mut result = if inherent.is_empty() {
            candidates
        } else {
            inherent
        };
        result.sort_by_key(|s| &s.id);
        result.dedup_by_key(|s| &s.id);
        result
    }
    pub(super) fn method_substitutions(
        &mut self,
        function: &Symbol,
        value: &ValueType,
        depth: usize,
    ) -> HashMap<String, ValueType> {
        if depth > 24 || !self.step() {
            return HashMap::new();
        }
        if let ValueType::Reference(t) = value {
            return self.method_substitutions(function, t, depth + 1);
        }
        if let ValueType::Standard(wrapper, args) = value {
            if matches!(wrapper.as_str(), "Box" | "Arc" | "Rc") {
                return self.method_substitutions(function, &args[0], depth + 1);
            }
        }
        let Some(d) = self.docs.get(&function.document_id) else {
            return HashMap::new();
        };
        let mut found = vec![];
        let default_trait = self
            .symbols
            .iter()
            .filter(|s| {
                s.document_id == function.document_id
                    && s.kind == "trait_item"
                    && s.start_byte <= function.start_byte
                    && function.end_byte <= s.end_byte
            })
            .min_by_key(|s| s.end_byte - s.start_byte);
        for d in self
            .docs
            .values()
            .filter(|candidate| candidate.project == d.project)
        {
            for implementation in &d.rust_semantics.trait_data.implementations {
                let from_default = if let (Some(declaration), Some(t)) =
                    (default_trait, &implementation.trait_type)
                {
                    !implementation
                        .methods
                        .iter()
                        .filter_map(|id| self.symbol(id))
                        .any(|s| s.name == function.name)
                        && matches!(self.types(d,implementation.start,"",t,&HashMap::new(),depth+1),ValueType::Nominal(ref id,_) if *id==declaration.id)
                } else {
                    false
                };
                if implementation.methods.contains(&function.id) || from_default {
                    if let Some(mut substitutions) =
                        self.impl_match(d, implementation, value, depth + 1)
                    {
                        for (name, t) in &implementation.associated {
                            let value = self.types(
                                d,
                                implementation.start,
                                "",
                                t,
                                &substitutions,
                                depth + 1,
                            );
                            substitutions.insert(format!("Self::{name}"), value);
                        }
                        found.push(substitutions);
                    }
                }
            }
        }
        if found.len() == 1 {
            found.pop().unwrap()
        } else {
            HashMap::new()
        }
    }
}
