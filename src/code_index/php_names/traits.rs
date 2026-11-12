//! Source-bound trait composition, not runtime trait execution or class synthesis.
use super::hierarchy::Hierarchy;
use super::*;
type Checked<T> = std::result::Result<T, &'static str>;
const MAX_GROUPS: usize = 64;
const MAX_TRAITS: usize = 64;
const MAX_RULES: usize = 256;
const MAX_DEPTH: usize = 32;
const WORK: usize = 4096;
fn names(node: Node) -> Vec<Node> {
    children(node)
        .into_iter()
        .filter(|n| n.kind() != "comment")
        .collect()
}
fn name(node: Node, source: &str) -> Option<String> {
    matches!(node.kind(), "name" | "qualified_name" | "relative_name")
        .then(|| plain(text(node, source)))
        .flatten()
        .map(|_| text(node, source).to_string())
}
fn member(node: Node, source: &str) -> Option<(Option<String>, String)> {
    if node.kind() == "name" {
        return Some((None, plain(text(node, source))?));
    }
    if node.kind() != "class_constant_access_expression" {
        return None;
    }
    let children = names(node);
    if children.len() != 2 {
        return None;
    }
    let owner = name(children[0], source)?;
    let method = name(children[1], source)?;
    (!method.contains('\\')).then_some((Some(owner), method))
}
fn rule(node: Node, source: &str) -> Option<TraitRule> {
    let children = names(node);
    let (owner, method) = member(*children.first()?, source)?;
    match node.kind() {
        "use_instead_of_clause" => {
            owner.as_ref()?;
            let excluded = children
                .iter()
                .skip(1)
                .map(|n| name(*n, source))
                .collect::<Option<Vec<_>>>()?;
            if excluded.is_empty() {
                return None;
            }
            Some(TraitRule {
                source: owner,
                method,
                excluded,
                alias: None,
                visibility: None,
            })
        }
        "use_as_clause" => {
            let mut alias = None;
            let mut visibility = None;
            for child in children.iter().skip(1) {
                // The locked grammar may classify PHP 8.3 `as final` as an alias.
                // Without a PHP-version-aware final adaptation, reject it explicitly.
                if text(*child, source).eq_ignore_ascii_case("final") {
                    return None;
                }
                if child.kind() == "visibility_modifier" && visibility.is_none() {
                    let value = text(*child, source).to_ascii_lowercase();
                    if !matches!(value.as_str(), "public" | "protected" | "private") {
                        return None;
                    }
                    visibility = Some(value);
                } else if child.kind() == "name" && alias.is_none() {
                    alias = Some(name(*child, source)?);
                } else {
                    return None;
                }
            }
            if alias.is_none() && visibility.is_none() {
                return None;
            }
            Some(TraitRule {
                source: owner,
                method,
                excluded: Vec::new(),
                alias,
                visibility,
            })
        }
        _ => None,
    }
}
pub(super) fn capture(node: Node, source: &str) -> Vec<TraitUse> {
    let Some(body) = node.child_by_field_name("body") else {
        return vec![];
    };
    let groups: Vec<_> = children(body)
        .into_iter()
        .filter(|n| n.kind() == "use_declaration")
        .collect();
    let groups_complete = groups.len() <= MAX_GROUPS;
    groups
        .into_iter()
        .take(MAX_GROUPS)
        .map(|group| {
            let mut captured = TraitUse {
                names: Vec::new(),
                rules: Vec::new(),
                line: group.start_position().row + 1,
                complete: groups_complete,
            };
            for n in names(group) {
                if n.kind() == "use_list" {
                    for clause in names(n) {
                        if let Some(rule) = rule(clause, source) {
                            captured.rules.push(rule)
                        } else {
                            captured.complete = false
                        }
                    }
                } else if let Some(name) = name(n, source) {
                    captured.names.push(name)
                } else {
                    captured.complete = false
                }
            }
            if captured.names.is_empty()
                || captured.names.len() > MAX_TRAITS
                || captured.rules.len() > MAX_RULES
            {
                captured.complete = false
            }
            captured.names.truncate(MAX_TRAITS);
            captured.rules.truncate(MAX_RULES);
            captured
        })
        .collect()
}
#[derive(Clone)]
pub(super) struct Effective<'a> {
    pub(super) definition: &'a Definition,
    pub(super) visibility: String,
    pub(super) host: &'a str,
    pub(super) from_trait: bool,
}
impl<'a> Effective<'a> {
    pub(super) fn declared(d: &'a Definition) -> Self {
        Self {
            definition: d,
            visibility: d.visibility.clone(),
            host: d.owner.as_deref().unwrap_or(""),
            from_trait: false,
        }
    }
}
fn spend(budget: &mut usize) -> Checked<()> {
    if *budget == 0 {
        return Err("PHP_TRAIT_COMPOSITION_WORK_LIMIT");
    }
    *budget -= 1;
    Ok(())
}
fn trait_target<'a>(h: &Hierarchy<'a>, scope: &Scope, raw: &str) -> Checked<&'a Definition> {
    let matches = h.named(scope, raw);
    if matches.is_empty() {
        return Err("PHP_TRAIT_TARGET_NOT_CAPTURED");
    }
    if matches.len() != 1 {
        return Err("PHP_TRAIT_IDENTITY_AMBIGUOUS");
    }
    let d = matches[0];
    if d.conditional {
        return Err("PHP_CONDITIONAL_TRAIT_UNVERIFIED");
    }
    if d.declaration_kind != "trait_declaration" {
        return Err("PHP_TRAIT_TARGET_KIND_INVALID");
    }
    Ok(d)
}
fn unique<'a>(mut candidates: Vec<Effective<'a>>) -> Checked<Option<Effective<'a>>> {
    candidates
        .sort_by(|a, b| (&a.definition.id, &a.visibility).cmp(&(&b.definition.id, &b.visibility)));
    candidates.dedup_by(|a, b| a.definition.id == b.definition.id && a.visibility == b.visibility);
    if candidates.len() > 1 {
        return Err("PHP_TRAIT_METHOD_CONFLICT");
    }
    Ok(candidates.pop())
}
fn provided<'a>(
    h: &Hierarchy<'a>,
    owner: &'a Definition,
    method: &str,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) -> Checked<Option<Effective<'a>>> {
    spend(budget)?;
    if let Some(d) = h.declared(&owner.id, method)? {
        return Ok(Some(Effective::declared(d)));
    }
    composed(h, owner, method, stack, budget)
}
fn composed<'a>(
    h: &Hierarchy<'a>,
    owner: &'a Definition,
    method: &str,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) -> Checked<Option<Effective<'a>>> {
    spend(budget)?;
    if !owner.uses_traits {
        return Ok(None);
    }
    if owner.trait_uses.is_empty() {
        return Err("PHP_TRAIT_CAPTURE_UNAVAILABLE");
    }
    if stack.len() >= MAX_DEPTH {
        return Err("PHP_TRAIT_COMPOSITION_DEPTH_LIMIT");
    }
    if !stack.insert(owner.id.clone()) {
        return Err("PHP_TRAIT_COMPOSITION_CYCLE");
    }
    let result = (|| {
        let scope = h
            .scopes
            .get(owner.id.as_str())
            .ok_or("PHP_SCOPE_IMPORT_TABLE_INCOMPLETE")?;
        let mut output = Vec::new();
        for group in &owner.trait_uses {
            spend(budget)?;
            if !group.complete {
                return Err("PHP_TRAIT_CAPTURE_INCOMPLETE");
            }
            let mut sources = Vec::new();
            for raw in &group.names {
                spend(budget)?;
                let d = trait_target(h, scope, raw)?;
                if sources.iter().any(|s: &&Definition| s.id == d.id) {
                    return Err("PHP_TRAIT_USE_DUPLICATE");
                }
                sources.push(d);
            }
            let source_id = |raw: &str| -> Checked<&'a str> {
                let d = trait_target(h, scope, raw)?;
                if !sources.iter().any(|s| s.id == d.id) {
                    return Err("PHP_TRAIT_RULE_SOURCE_NOT_USED");
                }
                Ok(&d.id)
            };
            // Aliases keep access to an explicitly named original, including excluded methods.
            let mut candidates = Vec::new();
            for d in &sources {
                if let Some(found) = provided(h, d, method, stack, budget)? {
                    candidates.push((d.id.as_str(), found))
                }
            }
            for rule in group
                .rules
                .iter()
                .filter(|r| !r.excluded.is_empty() && r.method.eq_ignore_ascii_case(method))
            {
                spend(budget)?;
                let chosen = source_id(rule.source.as_deref().ok_or("PHP_TRAIT_RULE_INVALID")?)?;
                if !candidates.iter().any(|(id, _)| *id == chosen) {
                    return Err("PHP_TRAIT_RULE_METHOD_NOT_CAPTURED");
                }
                for excluded in &rule.excluded {
                    let id = source_id(excluded)?;
                    if id == chosen {
                        return Err("PHP_TRAIT_RULE_SELF_EXCLUSION");
                    }
                    candidates.retain(|(source, _)| *source != id);
                }
            }
            // Visibility-only adaptations change the selected original, not other aliases.
            for rule in group.rules.iter().filter(|r| {
                r.excluded.is_empty() && r.alias.is_none() && r.method.eq_ignore_ascii_case(method)
            }) {
                spend(budget)?;
                let chosen = rule.source.as_deref().map(source_id).transpose()?;
                let matches: Vec<_> = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, (id, _))| chosen.is_none_or(|chosen| *id == chosen))
                    .map(|(i, _)| i)
                    .collect();
                if matches.len() != 1 {
                    return Err("PHP_TRAIT_ADAPTATION_AMBIGUOUS");
                }
                if let Some(visibility) = &rule.visibility {
                    candidates[matches[0]].1.visibility = visibility.clone()
                }
            }
            let mut selected: Vec<_> = candidates.into_iter().map(|(_, e)| e).collect();
            for rule in group.rules.iter().filter(|r| {
                r.alias
                    .as_ref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(method))
            }) {
                spend(budget)?;
                let chosen = rule.source.as_deref().map(source_id).transpose()?;
                let mut aliases = Vec::new();
                for d in sources
                    .iter()
                    .filter(|d| chosen.is_none_or(|id| d.id == id))
                {
                    if let Some(mut found) = provided(h, d, &rule.method, stack, budget)? {
                        if let Some(v) = &rule.visibility {
                            found.visibility = v.clone()
                        }
                        aliases.push((d.id.as_str(), found));
                    }
                }
                if chosen.is_none() {
                    for selection in group.rules.iter().filter(|r| {
                        !r.excluded.is_empty() && r.method.eq_ignore_ascii_case(&rule.method)
                    }) {
                        spend(budget)?;
                        let winner = source_id(
                            selection
                                .source
                                .as_deref()
                                .ok_or("PHP_TRAIT_RULE_INVALID")?,
                        )?;
                        if !aliases.iter().any(|(id, _)| *id == winner) {
                            return Err("PHP_TRAIT_RULE_METHOD_NOT_CAPTURED");
                        }
                        for raw in &selection.excluded {
                            let excluded = source_id(raw)?;
                            if excluded == winner {
                                return Err("PHP_TRAIT_RULE_SELF_EXCLUSION");
                            }
                            aliases.retain(|(id, _)| *id != excluded);
                        }
                    }
                }
                let alias = unique(aliases.into_iter().map(|(_, e)| e).collect())?
                    .ok_or("PHP_TRAIT_ALIAS_METHOD_NOT_CAPTURED")?;
                selected.push(alias);
            }
            output.extend(selected);
        }
        let result = unique(output)?;
        Ok(result.map(|mut d| {
            d.host = &owner.id;
            d.from_trait = true;
            d
        }))
    })();
    stack.remove(&owner.id);
    result
}
pub(super) fn method<'a>(
    h: &Hierarchy<'a>,
    owner: &'a Definition,
    name: &str,
) -> Checked<Option<Effective<'a>>> {
    let mut budget = WORK;
    composed(h, owner, name, &mut HashSet::new(), &mut budget)
}

pub(super) fn exposed<'a>(
    h: &Hierarchy<'a>,
    owner: &'a Definition,
    name: &str,
    budget: &mut usize,
) -> Checked<Option<Effective<'a>>> {
    provided(h, owner, name, &mut HashSet::new(), budget)
}
