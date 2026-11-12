use super::*;
pub(super) fn targets(scope: &Scope, raw: &str, domain: &str) -> (Vec<String>, bool) {
    let Some(name) = plain(raw) else {
        return (Vec::new(), false);
    };
    if raw.starts_with('\\') {
        return (vec![name], false);
    }
    let (head, tail) = name.split_once('\\').unwrap_or((&name, ""));
    if head.eq_ignore_ascii_case("namespace") && !tail.is_empty() {
        return (vec![qualify(&scope.name, tail)], false);
    }
    let kind = if tail.is_empty() { domain } else { "class" };
    let imports: Vec<_> = scope
        .imports
        .iter()
        .filter(|i| i.kind == kind && i.alias.eq_ignore_ascii_case(head))
        .collect();
    if !imports.is_empty() {
        return (
            imports
                .iter()
                .map(|i| {
                    if tail.is_empty() {
                        i.target.clone()
                    } else {
                        format!("{}\\{tail}", i.target)
                    }
                })
                .collect(),
            false,
        );
    }
    let mut names = vec![qualify(&scope.name, &name)];
    let fallback = domain == "function" && tail.is_empty() && !scope.name.is_empty();
    if fallback {
        names.push(name);
    }
    (names, fallback)
}
pub(super) fn document(
    documents: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<CallEdge> {
    let Some(facts) = &doc.php_names else {
        return Vec::new();
    };
    let definitions: Vec<_> = documents
        .values()
        .filter(|d| d.project == doc.project && !d.parse_errors)
        .filter_map(|d| d.php_names.as_ref())
        .filter(|f| f.complete)
        .flat_map(|f| {
            f.definitions
                .iter()
                .filter(|d| f.scopes.get(d.scope).is_some_and(|s| s.complete))
        })
        .collect();
    let hierarchy = super::hierarchy::Hierarchy::new(documents, doc);
    facts
        .uses
        .iter()
        .map(|u| {
            let mut candidates = Vec::new();
            let mut context_evidence = None;
            let scope = &facts.scopes[u.scope];
            let mut reason = "PHP_DYNAMIC_OR_UNSUPPORTED_NAME";
            if !facts.complete || doc.parse_errors {
                reason = "PHP_NAME_CAPTURE_UNAVAILABLE";
            } else if !scope.complete {
                reason = "PHP_SCOPE_IMPORT_TABLE_INCOMPLETE";
            } else if u.kind == "function" || u.kind == "type" {
                let domain = if u.kind == "function" {
                    "function"
                } else {
                    "class"
                };
                let (names, fallback) = targets(scope, &u.target, domain);
                if !names.is_empty() {
                    reason = if domain == "function" {
                        "PHP_FUNCTION_TARGET_NOT_CAPTURED"
                    } else {
                        "PHP_CLASS_TARGET_NOT_CAPTURED"
                    };
                }
                for (i, name) in names.iter().enumerate() {
                    let found: Vec<_> = definitions
                        .iter()
                        .copied()
                        .filter(|d| d.kind == domain && d.fqn.eq_ignore_ascii_case(name))
                        .collect();
                    if found.is_empty() {
                        continue;
                    }
                    if found.iter().any(|d| d.conditional) {
                        reason = "PHP_CONDITIONAL_DECLARATION_UNVERIFIED";
                        break;
                    }
                    candidates = found.iter().map(|d| d.id.clone()).collect();
                    reason = if fallback && i > 0 {
                        "PHP_GLOBAL_FUNCTION_FALLBACK_CANDIDATE_LOAD_ORDER_UNVERIFIED"
                    } else if domain == "class" {
                        "PHP_CLASS_NAME_CANDIDATE_AUTOLOAD_UNVERIFIED"
                    } else {
                        "PHP_FUNCTION_NAME_CANDIDATE_LOAD_ORDER_UNVERIFIED"
                    };
                    break;
                }
            } else if u.kind == "static" || u.kind == "member" {
                if let Some((bound, basis, evidence)) =
                    super::contexts::binding(&hierarchy, u, scope)
                {
                    candidates = bound;
                    reason = basis;
                    context_evidence = Some(evidence);
                } else {
                    let (bound, basis, target_owner) = hierarchy.method(u, scope);
                    candidates = bound;
                    reason = basis;
                    if let Some(owner) = target_owner {
                        context_evidence = Some(super::contexts::entry(&candidates, reason, owner));
                    }
                }
            }
            candidates.sort();
            candidates.dedup();
            CallEdge {
                context_evidence: context_evidence.clone(),
                reference: u.reference.clone(),
                resolution: if !candidates.is_empty()
                    && context_evidence
                        .as_ref()
                        .is_some_and(|e| e.contexts.len() != 1 || !e.enumeration_complete)
                {
                    "ambiguous"
                } else if candidates.len() == 1 {
                    "resolved_static"
                } else if candidates.len() > 1 {
                    "ambiguous"
                } else {
                    "unresolved"
                }
                .into(),
                candidates,
                provenance: "derived",
                resolution_basis: reason.into(),
            }
        })
        .collect()
}
