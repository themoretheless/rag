//! Bounded local reaching binding origins. Predicate/heap/compiler semantics are unproven.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::result::Result;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct Origin {
    pub position: Option<usize>,
    pub local: String,
}
impl Origin {
    fn unknown() -> Self {
        Self {
            position: None,
            local: String::new(),
        }
    }
}
type Values = BTreeSet<Origin>;
type Env = BTreeMap<String, Values>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Read {
    pub byte: usize,
    pub local: String,
    pub origins: Values,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Function {
    pub body: usize,
    pub end: usize,
    pub complete: bool,
    pub locals: Vec<String>,
    pub reads: Vec<Read>,
}
#[derive(Default)]
struct Loop {
    breaks: Vec<Env>,
    continues: Vec<Env>,
}
struct Engine<'a> {
    source: &'a str,
    bindings: &'a [Binding],
    scopes: &'a Facts,
    work: usize,
    reads: BTreeMap<(usize, String), Values>,
    loops: Vec<Loop>,
    record: bool,
}
fn unknown() -> Values {
    BTreeSet::from([Origin::unknown()])
}
impl Engine<'_> {
    fn spend(&mut self) -> Result<(), String> {
        if self.work == 0 {
            return Err("receiver_flow_budget_exhausted".into());
        }
        self.work -= 1;
        Ok(())
    }
    fn merge(&mut self, states: Vec<Env>) -> Result<Option<Env>, String> {
        if states.is_empty() {
            return Ok(None);
        }
        let names: BTreeSet<_> = states.iter().flat_map(|s| s.keys().cloned()).collect();
        let mut out = Env::new();
        for name in names {
            self.spend()?;
            let mut values = Values::new();
            for state in &states {
                values.extend(state.get(&name).cloned().unwrap_or_else(unknown));
            }
            if values.len() > 8 {
                return Err("receiver_origin_budget_exhausted".into());
            }
            out.insert(name, values);
        }
        Ok(Some(out))
    }
    fn expr(&mut self, n: Node, env: &Env, depth: usize) -> Result<(), String> {
        self.spend()?;
        if depth >= 64 {
            return Err("receiver_expression_depth_exhausted".into());
        }
        match n.kind() {
            "identifier" => {
                let name = norm(text(n, self.source));
                if self.record {
                    if let Some(values) = env.get(&name) {
                        let read = self.reads.entry((n.start_byte(), name)).or_default();
                        read.extend(values.clone());
                        if read.len() > 8 || self.reads.len() > 4096 {
                            return Err("receiver_read_budget_exhausted".into());
                        }
                    }
                }
            }
            "attribute" => {
                if let Some(object) = n.child_by_field_name("object") {
                    self.expr(object, env, depth + 1)?;
                }
            }
            "lambda" => {}
            _ => {
                for child in children(n) {
                    self.expr(child, env, depth + 1)?;
                }
            }
        }
        Ok(())
    }
    fn assignment(&mut self, node: Node, env: &mut Env, depth: usize) -> Result<Values, String> {
        self.spend()?;
        if depth >= 64 {
            return Err("receiver_assignment_depth_exhausted".into());
        }
        let left = node.child_by_field_name("left");
        let right = node.child_by_field_name("right");
        let Some(right) = right else {
            return Ok(unknown());
        };
        let alias = if right.kind() == "assignment" {
            self.assignment(right, env, depth + 1)?
        } else {
            self.expr(right, env, depth + 1)?;
            if right.kind() == "identifier" && node.child_by_field_name("type").is_none() {
                env.get(&norm(text(right, self.source)))
                    .cloned()
                    .unwrap_or_else(unknown)
            } else {
                unknown()
            }
        };
        let Some(left) = left.filter(|n| n.kind() == "identifier") else {
            let mut targets = left.into_iter().collect::<Vec<_>>();
            while let Some(target) = targets.pop() {
                self.spend()?;
                match target.kind() {
                    "identifier" => {
                        env.insert(norm(text(target, self.source)), unknown());
                    }
                    "pattern_list"
                    | "tuple_pattern"
                    | "list_pattern"
                    | "list_splat_pattern"
                    | "dictionary_splat_pattern" => targets.extend(children(target)),
                    _ => {}
                }
            }
            return Ok(unknown());
        };
        let name = norm(text(left, self.source));
        let values = if node.kind() == "augmented_assignment" {
            unknown()
        } else if node.child_by_field_name("type").is_some() || right.kind() == "call" {
            BTreeSet::from([Origin {
                position: Some(node.start_byte()),
                local: name.clone(),
            }])
        } else {
            alias
        };
        env.insert(name, values.clone());
        Ok(values)
    }
    fn block(&mut self, body: Node, mut env: Env, depth: usize) -> Result<Option<Env>, String> {
        if depth >= 32 {
            return Err("receiver_region_depth_exhausted".into());
        }
        for statement in children(body) {
            self.spend()?;
            match statement.kind() {
                "comment" | "pass_statement" => {}
                "expression_statement" => {
                    for expr in children(statement) {
                        if matches!(expr.kind(), "assignment" | "augmented_assignment") {
                            self.assignment(expr, &mut env, 0)?;
                        } else {
                            self.expr(expr, &env, 0)?;
                        }
                    }
                }
                "return_statement" | "raise_statement" => {
                    self.expr(statement, &env, 0)?;
                    return Ok(None);
                }
                "break_statement" | "continue_statement" => {
                    if let Some(frame) = self.loops.last_mut() {
                        if frame.breaks.len() + frame.continues.len() >= 128 {
                            return Err("receiver_loop_state_budget_exhausted".into());
                        }
                        if statement.kind() == "break_statement" {
                            frame.breaks.push(env);
                        } else {
                            frame.continues.push(env);
                        }
                    }
                    return Ok(None);
                }
                "if_statement" => {
                    let mut arms = vec![statement];
                    arms.extend(
                        children(statement)
                            .into_iter()
                            .filter(|n| matches!(n.kind(), "elif_clause" | "else_clause")),
                    );
                    let has_else = arms.last().is_some_and(|n| n.kind() == "else_clause");
                    let mut live = Vec::new();
                    for arm in arms {
                        if let Some(condition) = arm.child_by_field_name("condition") {
                            self.expr(condition, &env, 0)?;
                        }
                        if let Some(body) = arm
                            .child_by_field_name("consequence")
                            .or_else(|| arm.child_by_field_name("body"))
                        {
                            if let Some(state) = self.block(body, env.clone(), depth + 1)? {
                                live.push(state);
                            }
                        }
                    }
                    if !has_else {
                        live.push(env);
                    }
                    let Some(joined) = self.merge(live)? else {
                        return Ok(None);
                    };
                    env = joined;
                }
                "while_statement" | "for_statement" => {
                    let is_for = statement.kind() == "for_statement";
                    if is_for {
                        if let Some(right) = statement.child_by_field_name("right") {
                            self.expr(right, &env, 0)?;
                        }
                    }
                    let body = statement
                        .child_by_field_name("body")
                        .ok_or("receiver_loop_body_missing")?;
                    let initial = env.clone();
                    let mut header = env.clone();
                    let mut converged = false;
                    let record = self.record;
                    for _ in 0..16 {
                        self.record = false;
                        let mut iteration = header.clone();
                        if is_for {
                            if let Some(left) = statement
                                .child_by_field_name("left")
                                .filter(|n| n.kind() == "identifier")
                            {
                                iteration.insert(norm(text(left, self.source)), unknown());
                            }
                        }
                        self.loops.push(Loop::default());
                        let normal = self.block(body, iteration, depth + 1)?;
                        let frame = self.loops.pop().unwrap();
                        let mut states = vec![initial.clone()];
                        states.extend(normal);
                        states.extend(frame.continues);
                        let next = self.merge(states)?.unwrap();
                        if next == header {
                            converged = true;
                            break;
                        }
                        header = next;
                    }
                    self.record = record;
                    if !converged {
                        return Err("receiver_loop_fixed_point_exhausted".into());
                    }
                    if let Some(condition) = statement.child_by_field_name("condition") {
                        self.expr(condition, &header, 0)?;
                    }
                    let mut iteration = header.clone();
                    if is_for {
                        if let Some(left) = statement
                            .child_by_field_name("left")
                            .filter(|n| n.kind() == "identifier")
                        {
                            iteration.insert(norm(text(left, self.source)), unknown());
                        }
                    }
                    self.loops.push(Loop::default());
                    let _ = self.block(body, iteration, depth + 1)?;
                    let frame = self.loops.pop().unwrap();
                    let mut exits = frame.breaks;
                    if let Some(alternative) = statement.child_by_field_name("alternative") {
                        if let Some(body) = alternative.child_by_field_name("body") {
                            exits.extend(self.block(body, header, depth + 1)?);
                        }
                    } else {
                        exits.push(header);
                    }
                    let Some(joined) = self.merge(exits)? else {
                        return Ok(None);
                    };
                    env = joined;
                }
                "import_statement" | "import_from_statement" => {
                    for b in self
                        .bindings
                        .iter()
                        .filter(|b| b.declaration_at == Some(statement.start_byte()))
                    {
                        let name = norm(&b.local);
                        env.insert(
                            name.clone(),
                            BTreeSet::from([Origin {
                                position: Some(statement.start_byte()),
                                local: name,
                            }]),
                        );
                    }
                }
                "function_definition" | "class_definition" => {}
                _ => {
                    // Exceptional/other suites are not a type solver: preserve known
                    // reads before the region and invalidate potentially written locals.
                    let written: BTreeSet<_> = self
                        .bindings
                        .iter()
                        .filter(|b| {
                            b.declaration_at.is_some_and(|p| {
                                statement.start_byte() <= p && p < statement.end_byte()
                            })
                        })
                        .map(|b| norm(&b.local))
                        .chain(
                            self.scopes
                                .handler_aliases
                                .iter()
                                .filter(|(_, p, _, _)| {
                                    statement.start_byte() <= *p && *p < statement.end_byte()
                                })
                                .map(|(n, _, _, _)| n.clone()),
                        )
                        .collect();
                    let mut base = env.clone();
                    for name in &written {
                        base.insert(name.clone(), unknown());
                    }
                    let mut stack = vec![statement];
                    while let Some(n) = stack.pop() {
                        self.spend()?;
                        if n.kind() == "block" {
                            let _ = self.block(n, base.clone(), depth + 1)?;
                        } else {
                            stack.extend(children(n).into_iter().filter(|n| {
                                !matches!(n.kind(), "function_definition" | "class_definition")
                            }));
                        }
                    }
                    for name in written {
                        env.insert(name, unknown());
                    }
                }
            }
        }
        Ok(Some(env))
    }
}
pub(super) fn capture(
    root: Node,
    source: &str,
    bindings: &[Binding],
    facts: &Facts,
) -> Vec<Function> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut work = 100000;
    while let Some(node) = stack.pop() {
        if work == 0 || out.len() >= 512 {
            break;
        }
        work -= 1;
        if node.kind() == "function_definition" {
            if let Some(body) = node.child_by_field_name("body") {
                let mut env = Env::new();
                for b in bindings {
                    if b.declaration_at.is_some_and(|p| {
                        at(facts, p).is_some_and(|i| facts.scopes[i].start == node.start_byte())
                    }) {
                        env.insert(norm(&b.local), unknown());
                    }
                }
                for (name, p, _) in &facts.loop_targets {
                    if at(facts, *p).is_some_and(|i| facts.scopes[i].start == node.start_byte()) {
                        env.insert(name.clone(), unknown());
                    }
                }
                for (name, p, _, _) in &facts.handler_aliases {
                    if at(facts, *p).is_some_and(|i| facts.scopes[i].start == node.start_byte()) {
                        env.insert(name.clone(), unknown());
                    }
                }
                for (name, p) in &facts.annotations {
                    if at(facts, *p).is_some_and(|i| facts.scopes[i].start == node.start_byte()) {
                        env.insert(name.clone(), unknown());
                    }
                }
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    for b in bindings.iter().filter(|b| {
                        b.declaration_at.is_some_and(|p| {
                            parameters.start_byte() <= p && p < parameters.end_byte()
                        })
                    }) {
                        let name = norm(&b.local);
                        env.insert(
                            name.clone(),
                            BTreeSet::from([Origin {
                                position: b.declaration_at,
                                local: name,
                            }]),
                        );
                    }
                }
                let locals = env.keys().cloned().collect();
                let mut engine = Engine {
                    source,
                    bindings,
                    scopes: facts,
                    work: 16384,
                    reads: BTreeMap::new(),
                    loops: Vec::new(),
                    record: true,
                };
                let complete = engine.block(body, env, 0).is_ok();
                let reads = if complete {
                    engine
                        .reads
                        .into_iter()
                        .map(|((byte, local), origins)| Read {
                            byte,
                            local,
                            origins,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                out.push(Function {
                    body: body.start_byte(),
                    end: body.end_byte(),
                    complete,
                    locals,
                    reads,
                });
            }
        }
        stack.extend(children(node));
    }
    out
}

/// Resolve each reaching origin through the existing import/type/inheritance
/// resolver, then retain its candidate union and explicit unknown alternative.
pub(super) fn apply(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    edges: &mut [CallEdge],
) {
    let mut lookups = HashMap::new();
    for (id, doc) in docs {
        let mut reads = HashMap::new();
        if let Some(facts) = &doc.python_names {
            for function in &facts.receiver_flows {
                for read in &function.reads {
                    reads.insert((read.byte, read.local.as_str()), &read.origins);
                }
            }
        }
        lookups.insert(id.as_str(), reads);
    }
    let mut tasks = BTreeMap::<usize, Vec<Option<Binding>>>::new();
    let mut incomplete_indexes = BTreeSet::new();
    for (index, edge) in edges.iter().enumerate() {
        let reference = &edge.reference;
        let parts: Vec<_> = reference.expression.split('.').collect();
        if parts.len() != 2 {
            continue;
        }
        let head = parts[0];
        let doc = &docs[&reference.document_id];
        let origins = lookups[reference.document_id.as_str()].get(&(reference.start_byte, head));
        let incomplete = doc.python_names.as_ref().is_some_and(|f| {
            f.receiver_flows.iter().any(|fun| {
                !fun.complete
                    && fun.body <= reference.start_byte
                    && reference.start_byte < fun.end
                    && fun.locals.iter().any(|n| n == head)
            })
        });
        let variants = if incomplete {
            incomplete_indexes.insert(index);
            vec![None]
        } else if let Some(origins) = origins {
            origins
                .iter()
                .map(|origin| {
                    origin
                        .position
                        .and_then(|position| {
                            doc.bindings.iter().rev().find(|b| {
                                b.declaration_at == Some(position) && b.local == origin.local
                            })
                        })
                        .cloned()
                })
                .collect()
        } else {
            continue;
        };
        tasks.insert(index, variants);
    }
    let passes = tasks.values().map(Vec::len).max().unwrap_or(0);
    let mut candidates = BTreeMap::<usize, BTreeSet<String>>::new();
    let mut uncertain = BTreeSet::new();
    let mut bases = BTreeMap::new();
    for pass in 0..passes {
        let mut overrides = HashMap::new();
        let mut indexes = HashMap::new();
        for (&index, variants) in &tasks {
            let Some(variant) = variants.get(pass) else {
                continue;
            };
            let r = &edges[index].reference;
            let doc = &docs[&r.document_id];
            let overlay = overrides.entry(r.document_id.clone()).or_insert_with(|| {
                let names = doc.python_names.as_ref().map(|f| Facts {
                    format: f.format,
                    receiver_flow_format: f.receiver_flow_format,
                    receiver_flows: Vec::new(),
                    complete: f.complete,
                    scopes: f.scopes.clone(),
                    guarded: f.guarded.clone(),
                    loop_targets: f.loop_targets.clone(),
                    handler_aliases: f.handler_aliases.clone(),
                    assignments: f.assignments.clone(),
                    augmented: f.augmented.clone(),
                    annotations: f.annotations.clone(),
                });
                IndexedDocument {
                    version: doc.version,
                    hash: doc.hash.clone(),
                    revision: doc.revision,
                    path: doc.path.clone(),
                    parse_errors: doc.parse_errors,
                    bindings: doc.bindings.clone(),
                    project: doc.project.clone(),
                    exports: doc.exports.clone(),
                    bases: doc.bases.clone(),
                    blocked_members: doc.blocked_members.clone(),
                    binding_limitations: doc.binding_limitations.clone(),
                    python_names: names,
                    ..Default::default()
                }
            });
            let mut binding = variant.clone().unwrap_or(Binding {
                local: String::new(),
                target: None,
                ty: None,
                start: 0,
                end: 0,
                declared: 0,
                hoisted: false,
                rust_namespace: false,
                rust_extern: false,
                declaration_at: None,
                python_class_scope: None,
            });
            binding.local = r.expression.split('.').next().unwrap().to_string();
            binding.start = r.start_byte;
            binding.end = r.start_byte.saturating_add(1);
            binding.declared = r.start_byte;
            binding.hoisted = false;
            binding.python_class_scope = None;
            overlay.bindings.push(binding);
            overlay.references.push(r.clone());
            indexes.insert(
                (
                    r.document_id.clone(),
                    r.start_byte,
                    r.is_call,
                    r.expression.clone(),
                ),
                index,
            );
        }
        let selected = overrides.keys().cloned().collect();
        for resolved in
            resolve::references_selected_overrides(symbols, docs, Some(&selected), Some(&overrides))
        {
            let r = &resolved.reference;
            let index = indexes[&(
                r.document_id.clone(),
                r.start_byte,
                r.is_call,
                r.expression.clone(),
            )];
            if resolved.has_unknown_targets() {
                uncertain.insert(index);
            }
            bases.insert(index, resolved.resolution_basis);
            candidates
                .entry(index)
                .or_default()
                .extend(resolved.candidates);
        }
    }
    for &index in tasks.keys() {
        let edge = &mut edges[index];
        edge.candidates = candidates
            .remove(&index)
            .unwrap_or_default()
            .into_iter()
            .collect();
        edge.resolution = if edge.candidates.is_empty() {
            "unresolved"
        } else if edge.candidates.len() == 1 && !uncertain.contains(&index) {
            "resolved_static"
        } else {
            "ambiguous"
        }
        .into();
        edge.resolution_basis = if incomplete_indexes.contains(&index) {
            "python_receiver_flow_capture_incomplete".into()
        } else if uncertain.contains(&index) {
            "python_receiver_type_join_with_unknown".into()
        } else if tasks[&index].len() == 1 {
            bases
                .remove(&index)
                .unwrap_or_else(|| "receiver_type".into())
        } else {
            "python_receiver_type_join".into()
        };
    }
}
