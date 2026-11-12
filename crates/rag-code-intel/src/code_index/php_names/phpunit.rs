//! PHPUnit declaration candidates; package loading and runner eligibility unverified.
use super::*;
const TEST_CASE: &str = "PHPUnit\\Framework\\TestCase";
const TEST_ATTRIBUTE: &str = "PHPUnit\\Framework\\Attributes\\Test";
pub(super) fn attributes(node: Node, source: &str) -> (Vec<String>, bool) {
    let Some(root) = node.child_by_field_name("attributes") else {
        return (vec![], true);
    };
    let mut stack = vec![root];
    let mut names = Vec::new();
    let mut work = 0;
    while let Some(n) = stack.pop() {
        work += 1;
        if work > 512 || names.len() > 128 {
            return (vec![], false);
        }
        if n.kind() == "attribute" {
            if let Some(name) = children(n)
                .into_iter()
                .find(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_name"))
            {
                if plain(text(name, source)).is_none() {
                    return (vec![], false);
                }
                names.push(text(name, source).to_string());
            } else {
                return (vec![], false);
            }
        } else {
            stack.extend(children(n));
        }
    }
    names.sort();
    names.dedup();
    (names, true)
}

fn ancestry<'a>(
    h: &hierarchy::Hierarchy<'a>,
    start: &'a Definition,
) -> Option<Vec<&'a Definition>> {
    let mut chain = Vec::new();
    let mut current = start;
    let mut seen = HashSet::new();
    for _ in 0..64 {
        if !seen.insert(&current.id) || h.class(&current.id).is_err() {
            return None;
        }
        chain.push(current);
        let bases: Vec<_> = current
            .bases
            .iter()
            .filter(|b| b.relation == "extends")
            .collect();
        if bases.len() != 1 {
            return None;
        }
        let scope = h.scopes.get(current.id.as_str())?;
        let (names, _) = super::resolve::targets(scope, &bases[0].expression, "class");
        if names.len() != 1 {
            return None;
        }
        if names[0].eq_ignore_ascii_case(TEST_CASE) {
            return Some(chain);
        }
        let found = h.named(scope, &bases[0].expression);
        if found.len() != 1 || found[0].declaration_kind != "class_declaration" || found[0].is_final
        {
            return None;
        }
        current = found[0];
    }
    None
}
fn gather_names(
    h: &hierarchy::Hierarchy<'_>,
    owner: &Definition,
    methods: &HashMap<&str, Vec<&Definition>>,
    names: &mut std::collections::BTreeMap<String, String>,
    path: &mut HashSet<String>,
    work: &mut usize,
) -> Option<()> {
    if *work == 0 || path.len() >= 32 || !path.insert(owner.id.clone()) {
        return None;
    }
    *work -= 1;
    for method in methods.get(owner.id.as_str()).into_iter().flatten() {
        if *work == 0 || names.len() >= 1024 {
            return None;
        }
        *work -= 1;
        let name = method.fqn.rsplit("::").next()?;
        names
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| name.into());
    }
    let scope = h.scopes.get(owner.id.as_str())?;
    for group in &owner.trait_uses {
        if !group.complete {
            return None;
        }
        for rule in &group.rules {
            if *work == 0 || names.len() >= 1024 {
                return None;
            }
            *work -= 1;
            if let Some(alias) = &rule.alias {
                names
                    .entry(alias.to_ascii_lowercase())
                    .or_insert_with(|| alias.clone());
            }
        }
        for raw in &group.names {
            if *work == 0 {
                return None;
            }
            *work -= 1;
            let found = h.named(scope, raw);
            if found.len() != 1
                || found[0].conditional
                || found[0].declaration_kind != "trait_declaration"
            {
                return None;
            }
            gather_names(h, found[0], methods, names, path, work)?;
        }
    }
    path.remove(&owner.id);
    Some(())
}
pub(super) fn entrypoints(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<super::super::analysis::EntryPoint> {
    let Some(facts) = &doc.php_names else {
        return vec![];
    };
    if doc.parse_errors || !facts.complete || facts.format != 6 {
        return vec![];
    }
    let h = hierarchy::Hierarchy::new(docs, doc);
    let mut methods: HashMap<&str, Vec<&Definition>> = HashMap::new();
    for d in h.definitions.values().filter(|d| d.kind == "method") {
        if let Some(owner) = d.owner.as_deref() {
            methods.entry(owner).or_default().push(d);
        }
    }
    for values in methods.values_mut() {
        values.sort_by(|a, b| (&a.fqn, &a.id).cmp(&(&b.fqn, &b.id)));
    }
    let symbols: HashMap<_, _> = docs
        .values()
        .filter(|d| d.project == doc.project && !d.parse_errors)
        .flat_map(|d| d.symbols.iter().map(move |s| (s.id.as_str(), (s, d))))
        .collect();
    let mut hosts: Vec<_> = facts
        .definitions
        .iter()
        .filter(|d| d.declaration_kind == "class_declaration" && !d.is_abstract && !d.conditional)
        .collect();
    hosts.sort_by(|a, b| (&a.fqn, &a.id).cmp(&(&b.fqn, &b.id)));
    let mut result = Vec::new();
    for host in hosts.into_iter().take(128) {
        let Some(chain) = ancestry(&h, host) else {
            continue;
        };
        let mut work = 65536;
        let mut names = std::collections::BTreeMap::new();
        if chain.iter().any(|owner| {
            gather_names(
                &h,
                owner,
                &methods,
                &mut names,
                &mut HashSet::new(),
                &mut work,
            )
            .is_none()
        }) {
            continue;
        }
        for name in names.into_values() {
            let mut selected = None;
            let mut uncertain = false;
            for owner in &chain {
                match super::traits::exposed(&h, owner, &name, &mut work) {
                    Ok(Some(method)) => {
                        selected = Some(method);
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        uncertain = true;
                        break;
                    }
                }
            }
            if uncertain {
                continue;
            }
            let Some(method) = selected else {
                continue;
            };
            let d = method.definition;
            if method.visibility != "public" || !d.has_body || d.is_static || d.conditional {
                continue;
            }
            let Some(scope) = h.scopes.get(d.id.as_str()) else {
                continue;
            };
            let attributed = d.test_attributes_complete
                && d.test_attributes.iter().any(|raw| {
                    let (names, _) = super::resolve::targets(scope, raw, "class");
                    names.len() == 1 && names[0].eq_ignore_ascii_case(TEST_ATTRIBUTE)
                });
            let declared = d.fqn.rsplit("::").next().unwrap_or("");
            let effective_name = if name.eq_ignore_ascii_case(declared) {
                declared
            } else {
                &name
            };
            if !effective_name.starts_with("test") && !attributed {
                continue;
            }
            let Some((symbol, body_doc)) = symbols.get(d.id.as_str()) else {
                continue;
            };
            let Some((host_symbol, host_doc)) = symbols.get(host.id.as_str()) else {
                continue;
            };
            result.push(super::super::analysis::EntryPoint {
                test_context: Some(super::super::analysis::TestContext {
                    host_symbol_id: host.id.clone(),
                    lexical_owner_symbol_id: method.host.into(),
                    effective_method: effective_name.into(),
                    origin: match (method.from_trait, method.host == host.id) {
                        (true, true) => "trait",
                        (true, false) => "inherited_trait",
                        (false, true) => "declared",
                        (false, false) => "inherited_declared",
                    }
                    .into(),
                    host_document_id: host_symbol.document_id.clone(),
                    host_path: host_doc.path.clone(),
                    host_revision: host_doc.revision,
                    host_content_hash: host_doc.hash.clone(),
                    trait_body: method.from_trait,
                    framework_execution_verified: false,
                }),
                document_id: symbol.document_id.clone(),
                start_byte: symbol.start_byte,
                project: body_doc.project.clone(),
                revision: body_doc.revision,
                content_hash: body_doc.hash.clone(),
                symbol_id: Some(d.id.clone()),
                resolution: "resolved_static".into(),
                handler_expression: None,
                kind: "phpunit_test_candidate".into(),
                route: None,
                method: None,
                path: body_doc.path.clone(),
                line: symbol.start_line,
                provenance: "derived",
                reference_kind: Some(
                    if attributed {
                        "phpunit_test_attribute_candidate"
                    } else {
                        "phpunit_test_name_candidate"
                    }
                    .into(),
                ),
                request_method: None,
                request_method_basis: None,
            });
        }
    }
    result
}
