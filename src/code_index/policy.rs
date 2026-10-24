//! Permission-decision syntax traces. Computed returns remain unknown, never implicit allows.
use super::*;
use rmcp::schemars;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PolicySpec {
    symbol_id: String,
    #[serde(default)]
    implicit_tail: bool,
    decisions: Vec<Decision>,
    default_behavior: String,
    diagnostics: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Decision {
    line: usize,
    end_line: usize,
    outcome: String,
    expression: String,
    conditions: Vec<String>,
    cases: Vec<String>,
    syntax: String,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct PolicyQuery {
    /// Exact symbol ID, qualified name or unambiguous short name.
    pub symbol: String,
    pub path: Option<String>,
    pub project: Option<String>,
    /// Keep matching literal case arms and unconditional/default/unknown context.
    pub action: Option<String>,
    pub depth: Option<usize>,
    pub limit: Option<usize>,
}
fn children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn short(s: &str) -> String {
    s.chars().take(512).collect()
}
fn literal_case(label: &str) -> Option<&str> {
    let first = label.chars().next()?;
    if !matches!(first, '\'' | '"') || !label.ends_with(first) || label.len() < 2 {
        return None;
    }
    let value = &label[1..label.len() - 1];
    if value.contains([first, '\\', '$', '{', '}']) {
        return None;
    }
    Some(value)
}
fn callable(node: Node) -> bool {
    matches!(
        node.kind(),
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "method_declaration"
            | "arrow_function"
            | "function_expression"
            | "lambda"
            | "lambda_expression"
            | "closure_expression"
            | "anonymous_function"
            | "anonymous_function_creation_expression"
            | "generator_function"
    )
}
fn is_if(node: Node) -> bool {
    matches!(
        node.kind(),
        "if_statement" | "if_expression" | "elif_clause" | "else_if_clause"
    )
}
fn body(node: Node) -> Option<Node> {
    node.child_by_field_name("consequence")
        .or_else(|| node.child_by_field_name("body"))
}
fn alternatives(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.children_by_field_name("alternative", &mut c).collect()
}
fn unwrap(mut n: Node) -> Node {
    while matches!(
        n.kind(),
        "parenthesized_expression" | "expression_statement" | "else_clause"
    ) {
        let Some(next) = n
            .child_by_field_name("body")
            .or_else(|| children(n).into_iter().find(|n| !n.is_extra()))
        else {
            break;
        };
        n = next;
    }
    n
}
fn outcome(node: Node, source: &str) -> &'static str {
    let n = unwrap(node);
    if matches!(n.kind(), "boolean_literal" | "boolean" | "true" | "false") {
        match text(n, source).to_ascii_lowercase().as_str() {
            "true" => "allow",
            "false" => "deny",
            _ => "unknown",
        }
    } else {
        "unknown"
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Flow {
    Fall,
    Allow,
    Deny,
    Unknown,
    Nonreturn,
    Break,
}
fn value_flow(node: Node, source: &str) -> Flow {
    match outcome(node, source) {
        "allow" => Flow::Allow,
        "deny" => Flow::Deny,
        _ => Flow::Unknown,
    }
}
fn container(node: Node) -> bool {
    matches!(
        node.kind(),
        "block"
            | "statement_block"
            | "compound_statement"
            | "colon_block"
            | "switch_body"
            | "switch_block"
            | "case_statement"
            | "switch_case"
            | "default_statement"
            | "switch_default"
    )
}
fn control_unknown(node: Node) -> bool {
    matches!(
        node.kind(),
        "try_statement"
            | "try_expression"
            | "for_statement"
            | "for_expression"
            | "while_statement"
            | "while_expression"
            | "do_statement"
            | "foreach_statement"
            | "loop_expression"
            | "with_statement"
            | "yield_expression"
            | "await_expression"
    )
}
// This is the syntactic fallback: all explicit if guards false, switch/match default arms.
// It is not an evaluator for roles, side effects, exceptions or exhaustiveness.
fn fallback(node: Node, source: &str, depth: usize, implicit_tail: bool) -> Flow {
    if depth >= 128 {
        return Flow::Unknown;
    }
    if node.kind() == "expression_statement"
        && text(node, source).trim_end().ends_with(';')
        && children(node)
            .first()
            .is_some_and(|n| outcome(*n, source) != "unknown")
    {
        return Flow::Fall;
    }
    let n = unwrap(node);
    if callable(n) {
        return Flow::Fall;
    }
    if matches!(n.kind(), "return_statement" | "return_expression") {
        return children(n)
            .into_iter()
            .find(|n| !n.is_extra())
            .map_or(Flow::Unknown, |v| value_flow(v, source));
    }
    if matches!(
        n.kind(),
        "throw_statement" | "throw_expression" | "raise_statement"
    ) {
        return Flow::Nonreturn;
    }
    if control_unknown(n) {
        return Flow::Unknown;
    }
    if is_if(n) {
        let alts = alternatives(n);
        let Some(a) = alts.last().copied() else {
            return Flow::Fall;
        };
        return fallback(a, source, depth + 1, implicit_tail);
    }
    if matches!(n.kind(), "switch_statement" | "switch_expression") {
        let Some(b) = n.child_by_field_name("body") else {
            return Flow::Unknown;
        };
        let arms = children(b);
        let Some(start) = arms
            .iter()
            .position(|a| matches!(a.kind(), "switch_default" | "default_statement"))
        else {
            return Flow::Fall;
        };
        for arm in &arms[start..] {
            let f = fallback(*arm, source, depth + 1, implicit_tail);
            if f == Flow::Break {
                return Flow::Fall;
            }
            if f != Flow::Fall {
                return f;
            }
        }
        return Flow::Fall;
    }
    if n.kind() == "match_expression" {
        return Flow::Unknown;
    }
    if container(n) {
        let cs = children(n);
        for (i, c) in cs.iter().enumerate() {
            if n.child_by_field_name("value")
                .is_some_and(|v| v.id() == c.id())
            {
                continue;
            }
            if matches!(c.kind(), "break_statement" | "continue_statement") {
                return Flow::Break;
            }
            let f = fallback(*c, source, depth + 1, implicit_tail);
            if f != Flow::Fall {
                return f;
            }
            if implicit_tail && i + 1 == cs.len() && !text(*c, source).trim_end().ends_with(';') {
                let f = value_flow(*c, source);
                if f != Flow::Unknown {
                    return f;
                }
            }
        }
        return Flow::Fall;
    }
    // Rust tail literals and expression-bodied arrows only; other languages use explicit returns.
    if implicit_tail && matches!(n.kind(), "boolean_literal" | "boolean" | "true" | "false") {
        return value_flow(n, source);
    }
    Flow::Fall
}
fn terminal(node: Node, depth: usize) -> bool {
    if depth >= 128 {
        return false;
    }
    let n = unwrap(node);
    if matches!(
        n.kind(),
        "return_statement" | "return_expression" | "throw_statement" | "raise_statement"
    ) {
        return true;
    }
    if container(n) {
        return children(n)
            .into_iter()
            .find(|c| !c.is_extra())
            .is_some_and(|_| children(n).into_iter().any(|c| terminal(c, depth + 1)));
    }
    if is_if(n) {
        let alts = alternatives(n);
        return body(n).is_some_and(|b| terminal(b, depth + 1))
            && alts.last().is_some_and(|a| a.kind() == "else_clause")
            && alts.iter().all(|a| {
                if is_if(*a) {
                    body(*a).is_some_and(|b| terminal(b, depth + 1))
                } else {
                    terminal(*a, depth + 1)
                }
            });
    }
    false
}
fn decision(
    spec: &mut PolicySpec,
    node: Node,
    source: &str,
    conditions: &[String],
    cases: &[String],
    syntax: &str,
) {
    if spec.decisions.len() >= 10_000 {
        if !spec
            .diagnostics
            .iter()
            .any(|s| s == "POLICY_DECISION_BUDGET_EXCEEDED")
        {
            spec.diagnostics
                .push("POLICY_DECISION_BUDGET_EXCEEDED".into());
        }
        return;
    }
    spec.decisions.push(Decision {
        line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        outcome: outcome(node, source).into(),
        expression: short(text(node, source)),
        conditions: conditions.to_vec(),
        cases: cases.to_vec(),
        syntax: syntax.into(),
    });
}
fn visit(
    node: Node,
    source: &str,
    spec: &mut PolicySpec,
    conditions: &[String],
    cases: &[String],
    depth: usize,
) {
    if depth >= 128 {
        spec.diagnostics.push("POLICY_TREE_DEPTH_EXCEEDED".into());
        return;
    }
    if callable(node) {
        return;
    }
    if matches!(node.kind(), "return_statement" | "return_expression") {
        if let Some(value) = children(node).into_iter().find(|n| !n.is_extra()) {
            let n = unwrap(value);
            if matches!(n.kind(), "ternary_expression" | "conditional_expression") {
                let condition = n
                    .child_by_field_name("condition")
                    .map(|n| short(text(n, source)))
                    .unwrap_or("unknown".into());
                for (field, positive) in [
                    ("consequence", true),
                    ("body", true),
                    ("alternative", false),
                ] {
                    if let Some(v) = n.child_by_field_name(field) {
                        let mut ctx = conditions.to_vec();
                        ctx.push(if positive {
                            condition.clone()
                        } else {
                            format!("not ({condition})")
                        });
                        decision(spec, v, source, &ctx, cases, "conditional_return");
                    }
                }
            } else {
                decision(spec, value, source, conditions, cases, "explicit_return");
            }
        } else {
            spec.decisions.push(Decision {
                line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                outcome: "unknown".into(),
                expression: "<no value>".into(),
                conditions: conditions.to_vec(),
                cases: cases.to_vec(),
                syntax: "explicit_return".into(),
            });
        }
        return;
    }
    if is_if(node) {
        let cond = node
            .child_by_field_name("condition")
            .map(|n| short(text(n, source)))
            .unwrap_or("unknown".into());
        let mut yes = conditions.to_vec();
        yes.push(cond.clone());
        if let Some(b) = body(node) {
            visit(b, source, spec, &yes, cases, depth + 1);
        }
        let mut no = conditions.to_vec();
        no.push(format!("not ({cond})"));
        for alt in alternatives(node) {
            visit(alt, source, spec, &no, cases, depth + 1);
            if is_if(alt) {
                if let Some(c) = alt.child_by_field_name("condition") {
                    no.push(format!("not ({})", short(text(c, source))));
                }
            }
        }
        return;
    }
    if matches!(node.kind(), "switch_statement" | "switch_expression") {
        if let Some(b) = node.child_by_field_name("body") {
            let expression = node
                .child_by_field_name("value")
                .or_else(|| node.child_by_field_name("condition"))
                .map(|n| short(text(n, source)))
                .unwrap_or("switch".into());
            let mut carried = vec![];
            for arm in children(b) {
                if matches!(
                    arm.kind(),
                    "switch_case" | "case_statement" | "default_statement" | "switch_default"
                ) {
                    let label = arm
                        .child_by_field_name("value")
                        .map(|v| short(text(v, source)))
                        .unwrap_or("default".into());
                    carried.push(label);
                    let mut ctx = conditions.to_vec();
                    ctx.push(format!("{expression}: case {}", carried.join(" or ")));
                    let mut labels = cases.to_vec();
                    labels.extend(carried.clone());
                    visit(arm, source, spec, &ctx, &labels, depth + 1);
                    if terminal(arm, 0)
                        || children(arm).iter().any(|n| n.kind() == "break_statement")
                    {
                        carried.clear();
                    }
                }
            }
        }
        return;
    }
    let mut ctx = conditions.to_vec();
    if control_unknown(node) {
        spec.diagnostics
            .push(format!("UNMODELED_CONTROL_FLOW:{}", node.kind()));
        ctx.push(format!("unmodeled {}", node.kind()));
    }
    let cs = children(node);
    for (i, c) in cs.iter().enumerate() {
        if c.is_extra() {
            continue;
        }
        // Rust tail literal is a decision, not an arbitrary boolean subexpression.
        if spec.implicit_tail
            && container(node)
            && i + 1 == cs.len()
            && !text(*c, source).trim_end().ends_with(';')
            && outcome(*c, source) != "unknown"
        {
            decision(spec, *c, source, &ctx, cases, "implicit_tail");
        } else {
            visit(*c, source, spec, &ctx, cases, depth + 1);
        }
        if container(node) {
            if terminal(*c, 0) || matches!(c.kind(), "break_statement" | "continue_statement") {
                break;
            }
            if is_if(*c) && alternatives(*c).is_empty() && body(*c).is_some_and(|b| terminal(b, 0))
            {
                if let Some(cond) = c.child_by_field_name("condition") {
                    ctx.push(format!("not ({})", short(text(cond, source))));
                }
            }
        }
    }
}
pub(super) fn finish(root: Node, source: &str, out: &mut IndexedDocument) {
    for symbol in &out.symbols {
        let Some(mut node) =
            root.descendant_for_byte_range(symbol.start_byte, symbol.end_byte.saturating_sub(1))
        else {
            continue;
        };
        if node.kind() == "variable_declarator" {
            if let Some(v) = node.child_by_field_name("value") {
                node = v;
            }
        }
        if !callable(node) {
            continue;
        }
        let Some(b) = node.child_by_field_name("body") else {
            continue;
        };
        let implicit_tail = out.path.ends_with(".rs") || !container(b);
        let flow = fallback(b, source, 0, implicit_tail);
        let default = match flow {
            Flow::Allow => "allow",
            Flow::Deny => "deny",
            Flow::Nonreturn => "no_return",
            Flow::Fall => "implicit_return",
            Flow::Unknown | Flow::Break => "unknown",
        };
        let mut spec = PolicySpec {
            symbol_id: symbol.id.clone(),
            implicit_tail,
            decisions: vec![],
            default_behavior: default.into(),
            diagnostics: vec![],
        };
        if !container(b) && !is_if(b) {
            decision(&mut spec, b, source, &[], &[], "expression_body");
        } else {
            visit(b, source, &mut spec, &[], &[], 0);
        }
        if out.parse_errors {
            spec.default_behavior = "unknown".into();
            spec.diagnostics.push("SOURCE_PARSE_ERROR".into());
        }
        spec.diagnostics.sort();
        spec.diagnostics.dedup();
        out.policies.push(spec);
    }
}
impl CodeSnapshot {
    pub fn policy_trace(&self, q: &PolicyQuery) -> Result<serde_json::Value> {
        if q.symbol.trim().is_empty() {
            return Err(AppError::config("policy_trace requires a symbol"));
        }
        let matches: Vec<_> = self
            .symbols
            .iter()
            .filter(|s| {
                q.path.as_ref().is_none_or(|p| p == &s.path)
                    && (s.id == q.symbol || s.name == q.symbol || s.qualified_name == q.symbol)
                    && q.project.as_ref().is_none_or(|p| {
                        self.policy_projects
                            .get(&s.document_id)
                            .is_some_and(|v| v.as_ref() == Some(p))
                    })
            })
            .collect();
        if matches.len() != 1 {
            return Ok(
                serde_json::json!({"found":false,"ambiguous":matches.len()>1,"candidates":matches.iter().take(50).collect::<Vec<_>>(),"index_rev":self.index_revision,"stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents}),
            );
        }
        let target = matches[0];
        let Some(spec) = self.policies.iter().find(|p| p.symbol_id == target.id) else {
            return Ok(
                serde_json::json!({"found":false,"symbol":target,"reason":"unsupported_callable_body","index_rev":self.index_revision}),
            );
        };
        let limit = q.limit.unwrap_or(300).clamp(1, 500);
        let depth = q.depth.unwrap_or(3).clamp(1, 10);
        let selected: Vec<_> = spec
            .decisions
            .iter()
            .filter(|d| {
                q.action.as_ref().is_none_or(|a| {
                    d.cases.is_empty()
                        || d.cases.iter().any(|c| {
                            c == "default" || literal_case(c).is_none_or(|value| value == a)
                        })
                })
            })
            .collect();
        let mut budget = limit;
        let mut select = |outcome: &str| {
            let all: Vec<_> = selected
                .iter()
                .filter(|d| d.outcome == outcome)
                .copied()
                .collect();
            let items: Vec<_> = all.iter().take(budget).copied().collect();
            budget -= items.len();
            (items, all.len())
        };
        let (allows, allows_total) = select("allow");
        let (denies, denies_total) = select("deny");
        let (unknown, unknown_total) = select("unknown");
        let mut callers = vec![];
        let mut reached = HashSet::from([target.id.clone()]);
        let mut frontier = reached.clone();
        let mut caller_truncated = false;
        let mut seen_calls = HashSet::new();
        let mut depth_limited = false;
        for distance in 1..=depth {
            let mut next = HashSet::new();
            for edge in &self.calls {
                if edge.candidates.iter().any(|id| frontier.contains(id)) {
                    if !seen_calls.insert((
                        edge.reference.document_id.clone(),
                        edge.reference.start_byte,
                    )) {
                        continue;
                    }
                    if callers.len() >= limit {
                        caller_truncated = true;
                        continue;
                    }
                    callers.push(serde_json::json!({"reference":edge.reference,"candidates":edge.candidates,"resolution":edge.resolution,"depth":distance,"provenance":edge.provenance}));
                    if let Some(c) = &edge.reference.caller {
                        if reached.insert(c.clone()) {
                            next.insert(c.clone());
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            if distance == depth {
                depth_limited = true;
            }
            frontier = next;
        }
        let fail_closed = match spec.default_behavior.as_str() {
            "deny" => Some(true),
            "allow" => Some(false),
            _ => None,
        };
        Ok(
            serde_json::json!({"found":true,"index_rev":self.index_revision,"view":"fresh_subset_of_revision_inputs","symbol":target,"action_filter":q.action,"allows":allows,"denies":denies,"unknown":unknown,
            "decision_counts":{"allow":allows_total,"deny":denies_total,"unknown":unknown_total},"decisions_truncated":selected.len()>limit,"default_behavior":spec.default_behavior,
            "fallback_semantics":"all_explicit_if_guards_false_and_switch_default_arm","condition_scope":"syntactic_branches_and_prior_terminating_guards","fail_closed":fail_closed,"fail_closed_scope":"syntactic_fallback_only","callers":callers,"callers_truncated":caller_truncated,"caller_depth_limit_reached":depth_limited,"body_source":"verified_indexed_source",
            "diagnostics":spec.diagnostics,"binding_diagnostics":self.binding_diagnostics,"analysis_incomplete":unknown_total>0||spec.default_behavior=="unknown"||!spec.diagnostics.is_empty()||!self.binding_diagnostics.is_empty()||!self.stale_documents.is_empty()||!self.unindexed_documents.is_empty()||!self.parse_errors.is_empty(),
            "stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors,"not_checked":["runtime_authorization","role_values","condition_satisfiability","exceptions_and_side_effects","complete_control_flow","dynamic_dispatch"]}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut s = CodeSnapshot::default();
        for (i, (path, source)) in files.iter().enumerate() {
            let d = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!d.parse_errors, "{path} parse failure");
            s.symbols.extend(d.symbols.clone());
            s.policies.extend(d.policies.clone());
            s.policy_projects.insert(i.to_string(), None);
            docs.insert(i.to_string(), d);
        }
        s.calls = resolve::calls(&s.symbols, &docs);
        s
    }
    fn query(name: &str) -> PolicyQuery {
        PolicyQuery {
            symbol: name.into(),
            ..Default::default()
        }
    }
    #[test]
    fn conditional_denial_does_not_make_later_allow_fail_closed() {
        let s=snapshot(&[("p.py","def policy(role):\n    if role == 'blocked':\n        return False\n    return True\n\ndef handler():\n    return policy('employee')\n")]);
        let r = s.policy_trace(&query("policy")).unwrap();
        assert_eq!(r["default_behavior"], "allow", "{r}");
        assert_eq!(r["fail_closed"], false);
        assert_eq!(r["allows"].as_array().unwrap().len(), 1);
        assert_eq!(r["denies"].as_array().unwrap().len(), 1);
        assert!(r["allows"][0]["conditions"][0]
            .as_str()
            .unwrap()
            .contains("not (role"));
        assert_eq!(r["callers"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn computed_return_is_unknown_and_nested_functions_comments_are_excluded() {
        let s = snapshot(&[(
            "p.ts",
            r#"function policy(role: any) {
  function ignored() { return true; }
  const ignoredArrow = () => { return true; };
  // return true;
  if (role === 'admin') return true;
  return permissions[role];
}
function empty() { if (condition) return false; }
const arrow = () => false;
"#,
        )]);
        let r = s.policy_trace(&query("policy")).unwrap();
        assert_eq!(r["decision_counts"]["allow"], 1, "{r}");
        assert_eq!(r["decision_counts"]["unknown"], 1);
        assert_eq!(r["default_behavior"], "unknown");
        assert!(r["fail_closed"].is_null());
        assert_eq!(
            s.policy_trace(&query("empty")).unwrap()["default_behavior"],
            "implicit_return"
        );
        assert_eq!(
            s.policy_trace(&query("arrow")).unwrap()["default_behavior"],
            "deny"
        );
    }
    #[test]
    fn php_switch_cases_default_fallthrough_and_action_context() {
        let s = snapshot(&[(
            "policy.php",
            r#"<?php
function policy($action, $role) {
    switch ($action) {
        case 'approve':
        case 'pay':
            if ($role === 'admin') return true;
            return false;
        case 'view': return true;
        default: return false;
    }
}
function handler() { return policy('approve', 'employee'); }
function computed($x) { return $x; }
function broken($a) { switch ($a) { default: break; case 'view': return true; } return false; }
"#,
        )]);
        let mut q = query("policy");
        q.action = Some("approve".into());
        let r = s.policy_trace(&q).unwrap();
        assert_eq!(r["default_behavior"], "deny", "{r}");
        assert_eq!(r["decision_counts"]["allow"], 1, "{r}");
        assert_eq!(r["decision_counts"]["deny"], 2);
        assert_eq!(
            r["allows"][0]["cases"],
            serde_json::json!(["'approve'", "'pay'"])
        );
        assert_eq!(r["callers"].as_array().unwrap().len(), 1);
        assert_eq!(
            s.policy_trace(&query("computed")).unwrap()["decision_counts"]["unknown"],
            1
        );
        assert_eq!(
            s.policy_trace(&query("broken")).unwrap()["default_behavior"],
            "deny"
        );
    }
    #[test]
    fn boolean_literals_ternaries_tail_expressions_and_ambiguous_targets() {
        let s=snapshot(&[("p.rs","fn policy(admin: bool) -> bool { if admin { return true; } false }"),
        ("p.js","function choice(admin) { return admin ? true : false; } function policy() { return true; }")]);
        assert_eq!(s.policy_trace(&query("policy")).unwrap()["ambiguous"], true);
        let mut q = query("policy");
        q.path = Some("p.rs".into());
        let r = s.policy_trace(&q).unwrap();
        assert_eq!(r["default_behavior"], "deny", "{r}");
        assert_eq!(r["decision_counts"]["deny"], 1);
        let r = s.policy_trace(&query("choice")).unwrap();
        assert_eq!(r["decision_counts"]["allow"], 1);
        assert_eq!(r["decision_counts"]["deny"], 1);
        assert_eq!(r["default_behavior"], "unknown");
    }
    #[test]
    fn action_filter_keeps_computed_case_labels() {
        let s=snapshot(&[("p.php","<?php function policy($a,$b) {switch($a) {case 'prefix'.$b: return true; default: return false;}}")]);
        let mut q = query("policy");
        q.action = Some("approve".into());
        let r = s.policy_trace(&q).unwrap();
        assert_eq!(r["decision_counts"]["allow"], 1, "{r}");
        assert_eq!(r["decision_counts"]["deny"], 1);
    }

    #[test]
    fn bare_boolean_statements_are_not_return_values() {
        let s = snapshot(&[
            ("p.js", "function js() { false; }"),
            ("p.py", "def py():\n    False\n"),
            ("p.rs", "fn rs() { false; }"),
        ]);
        for name in ["js", "py", "rs"] {
            let r = s.policy_trace(&query(name)).unwrap();
            assert_eq!(r["default_behavior"], "implicit_return", "{r}");
            assert_eq!(r["decision_counts"]["deny"], 0, "{r}");
        }
    }

    #[test]
    fn php_methods_parse_but_namespace_calls_fail_closed() {
        let s=snapshot(&[("p.php", "<?php namespace One; function policy() {return false;} function handler() {return policy();} class Gate { public function allows() { return true; } }")]);
        assert!(s.calls.iter().all(|c| c.candidates.is_empty()));
        assert!(s
            .calls
            .iter()
            .any(|c| c.resolution_basis == "PHP_NAMESPACE_BINDING_UNMODELED"));
        let r = s.policy_trace(&query("Gate::allows")).unwrap();
        assert_eq!(r["default_behavior"], "allow", "{r}");
    }

    #[test]
    fn unmodeled_finally_and_loops_do_not_claim_fail_closed() {
        let s=snapshot(&[("p.js","function policy() { try { return false; } finally { return true; } } function loop() { while (x) { return true; } return false; }")]);
        for name in ["policy", "loop"] {
            let r = s.policy_trace(&query(name)).unwrap();
            assert_eq!(r["default_behavior"], "unknown", "{r}");
            assert_eq!(r["analysis_incomplete"], true);
            assert!(r["fail_closed"].is_null());
        }
    }
}
