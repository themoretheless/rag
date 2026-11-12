//! Captured lexical consumer contexts for trait bodies. Runtime subtype flow is unproven.
use super::hierarchy::Hierarchy;
use super::*;
const MAX_CONSUMERS: usize = 128;
const MAX_NAMES: usize = 128;
const MAX_DEPTH: usize = 32;
const WORK: usize = 8192;
#[derive(Clone)]
struct Consumer {
    owner: String,
    failure: Option<&'static str>,
}
#[derive(Clone)]
pub(super) struct Consumers {
    rows: Vec<Consumer>,
    complete: bool,
    truncated: bool,
}
fn scan(
    h: &Hierarchy<'_>,
    owner: &Definition,
    target: &str,
    names: &mut std::collections::BTreeSet<String>,
    path: &mut HashSet<String>,
    budget: &mut usize,
) -> (bool, bool, bool) {
    if *budget == 0 || path.len() >= MAX_DEPTH {
        return (false, false, true);
    }
    if !path.insert(owner.id.clone()) {
        return (false, false, false);
    }
    *budget -= 1;
    let mut found = owner.id == target;
    let mut complete = true;
    let mut limited = false;
    if let Some(scope) = h.scopes.get(owner.id.as_str()) {
        for group in &owner.trait_uses {
            complete &= group.complete;
            for rule in &group.rules {
                if let Some(alias) = &rule.alias {
                    if names.len() == MAX_NAMES && !names.contains(&alias.to_ascii_lowercase()) {
                        complete = false;
                        limited = true;
                    } else {
                        names.insert(alias.to_ascii_lowercase());
                    }
                }
            }
            for name in &group.names {
                if *budget == 0 {
                    complete = false;
                    limited = true;
                    break;
                }
                *budget -= 1;
                let matches = h.named(scope, name);
                if matches.len() != 1
                    || matches[0].declaration_kind != "trait_declaration"
                    || matches[0].conditional
                {
                    complete = false;
                    continue;
                }
                let (nested, nested_complete, nested_limited) =
                    scan(h, matches[0], target, names, path, budget);
                found |= nested;
                complete &= nested_complete;
                limited |= nested_limited;
            }
        }
    } else {
        complete = false
    }
    path.remove(&owner.id);
    (found, complete, limited)
}
fn discover(h: &Hierarchy<'_>, caller: &Definition, trait_id: &str) -> Consumers {
    let mut hosts: Vec<_> = h
        .definitions
        .values()
        .copied()
        .filter(|d| {
            d.uses_traits
                && matches!(
                    d.declaration_kind.as_str(),
                    "class_declaration" | "enum_declaration"
                )
        })
        .collect();
    hosts.sort_by(|a, b| (&a.fqn, &a.id).cmp(&(&b.fqn, &b.id)));
    let mut result = Consumers {
        rows: Vec::new(),
        complete: hosts.len() <= MAX_CONSUMERS,
        truncated: hosts.len() > MAX_CONSUMERS,
    };
    let mut budget = WORK;
    for host in hosts.into_iter().take(MAX_CONSUMERS) {
        let mut names = std::collections::BTreeSet::from([caller
            .fqn
            .rsplit("::")
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()]);
        let (reachable, complete, limited) = scan(
            h,
            host,
            trait_id,
            &mut names,
            &mut HashSet::new(),
            &mut budget,
        );
        result.complete &= complete;
        result.truncated |= limited;
        if budget == 0 {
            result.truncated = true;
            break;
        }
        if !reachable {
            continue;
        }
        if !complete {
            result.rows.push(Consumer {
                owner: host.id.clone(),
                failure: Some("PHP_TRAIT_CONSUMER_COMPOSITION_INCOMPLETE"),
            });
            continue;
        }
        let mut retained = false;
        let mut failure = None;
        for name in names {
            match super::traits::exposed(h, host, &name, &mut budget) {
                Ok(Some(method)) if method.definition.id == caller.id => {
                    retained = true;
                    break;
                }
                Err(reason) => failure = Some(reason),
                _ => {}
            }
            if budget == 0 {
                result.complete = false;
                result.truncated = true;
                failure = Some("PHP_TRAIT_CONSUMER_WORK_LIMIT");
                break;
            }
        }
        if retained || failure.is_some() {
            if !retained {
                result.complete = false
            }
            result.rows.push(Consumer {
                owner: host.id.clone(),
                failure: if retained { None } else { failure },
            });
        }
    }
    result
}
pub(super) fn required_sites(doc: &IndexedDocument) -> HashSet<(usize, bool, String)> {
    let Some(f) = &doc.php_names else {
        return HashSet::new();
    };
    if !f.complete || doc.parse_errors {
        return HashSet::new();
    }
    let owners: HashSet<_> = f
        .definitions
        .iter()
        .filter(|d| d.declaration_kind == "trait_declaration")
        .map(|d| d.id.as_str())
        .collect();
    f.uses
        .iter()
        .filter(|u| {
            matches!(u.kind.as_str(), "static" | "member")
                && f.scopes.get(u.scope).is_some_and(|s| s.complete)
                && u.owner.as_deref().is_some_and(|id| owners.contains(id))
        })
        .map(|u| {
            (
                u.reference.start_byte,
                u.reference.is_call,
                u.reference.expression.clone(),
            )
        })
        .collect()
}
pub(super) fn binding(
    h: &Hierarchy<'_>,
    u: &Use,
    scope: &Scope,
) -> Option<(Vec<String>, &'static str, BindingContexts)> {
    let owner = u
        .owner
        .as_deref()
        .and_then(|id| h.definitions.get(id).copied())?;
    if owner.declaration_kind != "trait_declaration" {
        return None;
    }
    let caller = u
        .reference
        .caller
        .as_deref()
        .and_then(|id| h.definitions.get(id).copied());
    let consumers = if let Some(caller) = caller {
        let cached = h.context_cache.borrow().get(&caller.id).cloned();
        if let Some(cached) = cached {
            cached
        } else {
            let consumers = discover(h, caller, &owner.id);
            h.context_cache
                .borrow_mut()
                .insert(caller.id.clone(), consumers.clone());
            consumers
        }
    } else {
        Consumers {
            rows: Vec::new(),
            complete: false,
            truncated: false,
        }
    };
    let mut evidence = BindingContexts {
        kind: "php_captured_trait_consumers".into(),
        enumeration_complete: consumers.complete,
        truncated: consumers.truncated,
        runtime_contexts_complete: false,
        context_correlation_proven: false,
        contexts: Vec::new(),
    };
    let mut candidates = Vec::new();
    for consumer in consumers.rows {
        let (mut targets, reason, target_owner) = if let Some(failure) = consumer.failure {
            (Vec::new(), failure, None)
        } else {
            let mut rebound = u.clone();
            rebound.owner = Some(consumer.owner.clone());
            h.method(&rebound, scope)
        };
        targets.sort();
        targets.dedup();
        candidates.extend(targets.iter().cloned());
        evidence.contexts.push(BindingContext {
            owner: consumer.owner,
            target_owner,
            candidates: targets.clone(),
            resolution: match targets.len() {
                0 => "unresolved",
                1 => "resolved_static",
                _ => "ambiguous",
            }
            .into(),
            resolution_basis: reason.into(),
        });
    }
    evidence.contexts.sort_by(|a, b| a.owner.cmp(&b.owner));
    candidates.sort();
    candidates.dedup();
    let reason = if candidates.is_empty() {
        if evidence.contexts.is_empty() {
            "PHP_TRAIT_BODY_NO_CAPTURED_CONSUMERS"
        } else {
            "PHP_TRAIT_BODY_CONSUMER_BINDINGS_UNRESOLVED"
        }
    } else if evidence.contexts.len() == 1 && evidence.enumeration_complete {
        "PHP_TRAIT_BODY_SINGLE_CAPTURED_CONTEXT_RUNTIME_UNVERIFIED"
    } else {
        "PHP_TRAIT_BODY_CONTEXT_CANDIDATES_CORRELATION_UNVERIFIED"
    };
    Some((candidates, reason, evidence))
}

pub(super) fn allowed_ids<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> ContextIds<'a> {
    let mut owners = HashSet::new();
    let mut methods = HashSet::new();
    let mut trait_methods = HashSet::new();
    for facts in docs
        .values()
        .filter(|d| d.project == doc.project && !d.parse_errors)
        .filter_map(|d| d.php_names.as_ref())
        .filter(|f| f.format == 6 && f.complete)
    {
        let traits: HashSet<_> = facts
            .definitions
            .iter()
            .filter(|d| d.declaration_kind == "trait_declaration")
            .map(|d| d.id.as_str())
            .collect();
        for d in facts
            .definitions
            .iter()
            .filter(|d| facts.scopes.get(d.scope).is_some_and(|s| s.complete))
        {
            if matches!(
                d.declaration_kind.as_str(),
                "class_declaration" | "enum_declaration"
            ) {
                owners.insert(d.id.as_str());
            }
            if d.kind == "method" && d.has_body && !d.conditional {
                methods.insert(d.id.as_str());
                if d.owner.as_deref().is_some_and(|id| traits.contains(id)) {
                    trait_methods.insert(d.id.as_str());
                }
            }
        }
    }
    ContextIds {
        owners,
        methods,
        trait_methods,
    }
}

pub(super) fn entry(candidates: &[String], reason: &str, owner: String) -> BindingContexts {
    BindingContexts {
        kind: "php_captured_trait_entry".into(),
        enumeration_complete: true,
        truncated: false,
        runtime_contexts_complete: false,
        context_correlation_proven: false,
        contexts: vec![BindingContext {
            owner: owner.clone(),
            target_owner: Some(owner),
            candidates: candidates.to_vec(),
            resolution: if candidates.len() == 1 {
                "resolved_static"
            } else {
                "ambiguous"
            }
            .into(),
            resolution_basis: reason.into(),
        }],
    }
}
