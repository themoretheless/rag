use super::*;
mod completion;
fn children(n: Node<'_>) -> Vec<Node<'_>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}
#[derive(Clone)]
struct FlowState {
    locals: HashMap<String, String>,
    controls: Vec<String>,
    exception_id: Option<String>,
}
struct ReturnState {
    output: String,
    state: FlowState,
}
#[derive(Default)]
struct LoopFrame {
    breaks: Vec<FlowState>,
    continues: Vec<FlowState>,
}
struct Builder<'a> {
    source: &'a str,
    document: &'a str,
    function: Function,
    locals: HashMap<String, String>,
    controls: Vec<String>,
    loops: Vec<LoopFrame>,
    return_states: Vec<ReturnState>,
    protected: Vec<Vec<FlowState>>,
    flow_context: Vec<String>,
    next_finally: usize,
    active_exception: Vec<String>,
    work: usize,
    blocked: &'a HashSet<String>,
    module_calls_qualified: bool,
}
impl Builder<'_> {
    fn spend(&mut self) -> std::result::Result<(), String> {
        if self.work == 0 || self.function.nodes.len() >= 4096 || self.function.edges.len() >= 8192
        {
            return Err("capture_budget_exhausted".into());
        }
        self.work -= 1;
        Ok(())
    }
    fn node(
        &mut self,
        n: Node,
        kind: &str,
        name: Option<String>,
    ) -> std::result::Result<String, String> {
        self.spend()?;
        let mut id = format!(
            "{}:{}:{}:{}",
            self.document,
            n.start_byte(),
            n.end_byte(),
            kind
        );
        if !self.flow_context.is_empty() {
            id.push('@');
            id.push_str(&content_hash(
                &serde_json::to_string(&self.flow_context).unwrap(),
            ));
        }
        if self.controls.len() > 64 {
            return Err("control_depth_exhausted".into());
        }
        for condition in self.controls.clone() {
            self.edge(condition, &id, "control_dependency");
        }
        self.function.nodes.push(NodeFact {
            id: id.clone(),
            function: self.function.symbol.clone(),
            start_byte: n.start_byte(),
            end_byte: n.end_byte(),
            line: n.start_position().row + 1,
            end_line: n.end_position().row + 1,
            kind: kind.into(),
            flow_context: self.flow_context.clone(),
            binding_name: name.as_deref().map(binding_name),
            name,
        });
        Ok(id)
    }
    fn edge(&mut self, source: String, target: &str, kind: &str) {
        self.function.edges.push(FlowEdge {
            source,
            target: target.into(),
            kind: kind.into(),
        });
    }
    fn expr(&mut self, n: Node, depth: usize) -> std::result::Result<String, String> {
        self.spend()?;
        if depth >= 32 {
            return Err("expression_depth_exhausted".into());
        }
        match n.kind() {
            "identifier" => {
                let name = text(n, self.source).to_string();
                let definition = self
                    .locals
                    .get(&binding_name(&name))
                    .cloned()
                    .ok_or_else(|| format!("unbound_or_nonlocal_read:{name}"))?;
                let read = self.node(n, "read", Some(name))?;
                self.edge(definition, &read, "def_use");
                Ok(read)
            }
            "integer" | "float" | "true" | "false" | "none" => self.node(n, "literal", None),
            "string" => {
                if children(n).iter().any(|n| n.kind() == "interpolation") {
                    return Err("formatted_string_unverified".into());
                }
                self.node(n, "literal", None)
            }
            "binary_operator"
            | "unary_operator"
            | "comparison_operator"
            | "parenthesized_expression" => {
                let result = self.node(n, "expression", None)?;
                for child in children(n).into_iter().filter(|n| n.kind() != "comment") {
                    let input = self.expr(child, depth + 1)?;
                    self.edge(input, &result, "expression_input");
                }
                if n.kind() != "parenthesized_expression" {
                    self.exception(n, "operation", std::slice::from_ref(&result))?;
                }
                Ok(result)
            }
            "call" => {
                let target = n
                    .child_by_field_name("function")
                    .ok_or("call_target_missing")?;
                if target.kind() != "identifier"
                    || self
                        .locals
                        .contains_key(&binding_name(text(target, self.source)))
                {
                    return Err("dynamic_or_shadowed_call_unverified".into());
                }
                let args = n
                    .child_by_field_name("arguments")
                    .ok_or("arguments_missing")?;
                let result = self.node(n, "call_result", None)?;
                let mut arguments = Vec::new();
                for arg in children(args).into_iter().filter(|n| n.kind() != "comment") {
                    if arguments.len() >= 256 {
                        return Err("argument_budget_exhausted".into());
                    }
                    if matches!(
                        arg.kind(),
                        "keyword_argument"
                            | "list_splat"
                            | "dictionary_splat"
                            | "generator_expression"
                    ) {
                        return Err("nonpositional_call_unverified".into());
                    }
                    arguments.push(self.expr(arg, depth + 1)?);
                }
                self.exception(n, "call", &arguments)?;
                self.function.calls.push(Call {
                    binding_qualified: self.module_calls_qualified
                        && !self
                            .blocked
                            .contains(&binding_name(text(target, self.source))),
                    start_byte: n.start_byte(),
                    expression: text(target, self.source).into(),
                    arguments,
                    result: result.clone(),
                });
                Ok(result)
            }
            kind => Err(format!("unsupported_expression:{kind}")),
        }
    }

    fn branch(&mut self, node: Node, depth: usize) -> std::result::Result<bool, String> {
        if depth >= 32 {
            return Err("branch_depth_exhausted".into());
        }
        let outer = self.controls.clone();
        let initial = self.locals.clone();
        let mut live = Vec::new();
        let mut predicates = Vec::new();
        let mut terminated = false;
        let mut arms = vec![node];
        arms.extend(
            children(node)
                .into_iter()
                .filter(|n| matches!(n.kind(), "elif_clause" | "else_clause")),
        );
        let has_else = arms.last().is_some_and(|n| n.kind() == "else_clause");
        for arm in arms {
            self.spend()?;
            self.locals = initial.clone();
            self.controls = outer.iter().chain(&predicates).cloned().collect();
            let body = if arm.kind() == "else_clause" {
                arm.child_by_field_name("body").ok_or("else_body_missing")?
            } else {
                let condition = arm
                    .child_by_field_name("condition")
                    .ok_or("condition_missing")?;
                let value = self.expr(condition, 0)?;
                self.exception(condition, "truth", std::slice::from_ref(&value))?;
                predicates.push(value.clone());
                self.controls.push(value);
                arm.child_by_field_name("consequence")
                    .ok_or("consequence_missing")?
            };
            if self.block(body, depth)? {
                terminated = true;
            } else {
                live.push((self.locals.clone(), self.controls.clone()));
            }
        }
        if !has_else {
            live.push((initial, outer.iter().chain(&predicates).cloned().collect()));
        }
        self.controls = outer;
        if live.is_empty() {
            return Ok(true);
        }
        // Keep only definitely initialized locals from paths that can continue.
        // A returned arm cannot supply a later reaching definition.
        let mut keys: Vec<_> = live[0].0.keys().cloned().collect();
        keys.sort();
        self.locals.clear();
        for key in keys {
            self.spend()?;
            let inputs: BTreeSet<_> = live
                .iter()
                .filter_map(|(locals, _)| locals.get(&key).cloned())
                .collect();
            if live.iter().any(|(locals, _)| !locals.contains_key(&key)) {
                continue;
            }
            let definition = if inputs.len() == 1 {
                inputs.into_iter().next().unwrap()
            } else {
                let id = self.node(node, &format!("merge:{key}"), None)?;
                self.function.nodes.last_mut().unwrap().kind = "merge".into();
                for input in inputs {
                    self.edge(input, &id, "merge_input");
                }
                for condition in &predicates {
                    self.edge(condition.clone(), &id, "control_dependency");
                }
                id
            };
            self.locals.insert(key, definition);
        }
        for (_, controls) in live {
            for control in controls {
                if (terminated || !predicates.contains(&control))
                    && !self.controls.contains(&control)
                {
                    self.controls.push(control);
                }
            }
        }
        Ok(false)
    }
    fn state(&mut self) -> std::result::Result<FlowState, String> {
        for _ in 0..self.locals.len() + self.controls.len() {
            self.spend()?;
        }
        Ok(FlowState {
            locals: self.locals.clone(),
            controls: self.controls.clone(),
            exception_id: None,
        })
    }
    fn jump(&mut self, node: Node) -> std::result::Result<(), String> {
        if self.loops.is_empty() {
            return Err("jump_outside_loop".into());
        }
        let _ = self.node(node, node.kind(), None)?;
        let state = self.state()?;
        let frame = self.loops.last_mut().unwrap();
        if frame.breaks.len() + frame.continues.len() >= 128 {
            return Err("loop_state_budget_exhausted".into());
        }
        if node.kind() == "break_statement" {
            frame.breaks.push(state);
        } else {
            frame.continues.push(state);
        }
        Ok(())
    }
    fn loop_body(&mut self, node: Node, depth: usize) -> std::result::Result<bool, String> {
        if depth >= 32 || self.loops.len() >= 32 {
            return Err("loop_depth_exhausted".into());
        }
        let outer = self.controls.clone();
        let initial = self.locals.clone();
        let body = node
            .child_by_field_name("body")
            .ok_or("loop_body_missing")?;
        let is_for = node.kind() == "for_statement";
        if node.child(0).is_some_and(|n| n.kind() == "async") {
            return Err("async_loop_unverified".into());
        }
        let iterable = if is_for {
            Some(
                self.expr(
                    node.child_by_field_name("right")
                        .ok_or("iterable_missing")?,
                    0,
                )?,
            )
        } else {
            None
        };
        let mut written = BTreeSet::new();
        let mut scan = vec![body];
        while let Some(n) = scan.pop() {
            self.spend()?;
            if matches!(n.kind(), "assignment" | "for_statement") {
                if let Some(left) = n
                    .child_by_field_name("left")
                    .filter(|n| n.kind() == "identifier")
                {
                    written.insert(binding_name(text(left, self.source)));
                }
            }
            scan.extend(children(n));
        }
        let target = if is_for {
            let left = node
                .child_by_field_name("left")
                .ok_or("iteration_target_missing")?;
            if left.kind() != "identifier" {
                return Err("iteration_pattern_unverified".into());
            }
            written.insert(binding_name(text(left, self.source)));
            Some(left)
        } else {
            None
        };
        let mut headers = BTreeMap::new();
        for name in written {
            self.spend()?;
            if let Some(value) = initial.get(&name) {
                let header = self.node(node, &format!("loop_header:{name}"), None)?;
                self.function.nodes.last_mut().unwrap().kind = "loop_header".into();
                self.edge(value.clone(), &header, "loop_entry");
                headers.insert(name.clone(), header.clone());
                self.locals.insert(name, header);
            }
        }
        let header_locals = self.locals.clone();
        let condition = if let Some(iterable) = &iterable {
            let control = self.node(node, "iteration_control", None)?;
            self.edge(iterable.clone(), &control, "iteration_control_input");
            control
        } else {
            self.expr(
                node.child_by_field_name("condition")
                    .ok_or("condition_missing")?,
                0,
            )?
        };
        if is_for {
            self.exception(node, "iteration", std::slice::from_ref(&condition))?;
        } else {
            self.exception(
                node.child_by_field_name("condition").unwrap(),
                "truth",
                std::slice::from_ref(&condition),
            )?;
        }
        for header in headers.values() {
            self.edge(condition.clone(), header, "control_dependency");
        }
        self.controls.push(condition.clone());
        if let Some(left) = target {
            let item = self.node(left, "iteration_item", None)?;
            self.edge(iterable.unwrap(), &item, "iteration_value");
            let name = text(left, self.source).to_string();
            let definition = self.node(left, "definition", Some(name.clone()))?;
            self.edge(item, &definition, "assignment");
            self.locals.insert(binding_name(&name), definition);
        }
        self.loops.push(LoopFrame::default());
        let terminal = self.block(body, depth)?;
        let mut frame = self.loops.pop().unwrap();
        if !terminal {
            frame.continues.push(self.state()?);
        }
        for state in frame.continues {
            for (name, header) in &headers {
                self.spend()?;
                let value = state
                    .locals
                    .get(name)
                    .ok_or("loop_binding_not_initialized")?;
                if value != header {
                    self.edge(value.clone(), header, "loop_backedge");
                    for control in &state.controls {
                        self.edge(control.clone(), header, "control_dependency");
                    }
                }
            }
        }
        // Zero iterations are always retained; no predicate or iterator cardinality proof.
        self.locals = header_locals;
        self.controls = outer.clone();
        let mut normal_terminal = false;
        if let Some(alternative) = node.child_by_field_name("alternative") {
            self.controls.push(condition.clone());
            normal_terminal = self.block(
                alternative
                    .child_by_field_name("body")
                    .ok_or("loop_else_missing")?,
                depth,
            )?;
        }
        let mut exits = frame.breaks;
        if !normal_terminal {
            exits.push(self.state()?);
        }
        self.controls = outer;
        if exits.is_empty() {
            return Ok(true);
        }
        let mut keys: Vec<_> = exits[0].locals.keys().cloned().collect();
        keys.sort();
        self.locals.clear();
        for name in keys {
            self.spend()?;
            if exits.iter().any(|s| !s.locals.contains_key(&name)) {
                continue;
            }
            let inputs: BTreeSet<_> = exits.iter().map(|s| s.locals[&name].clone()).collect();
            let value = if inputs.len() == 1 {
                inputs.into_iter().next().unwrap()
            } else {
                let merge = self.node(node, &format!("loop_exit:{name}"), None)?;
                self.function.nodes.last_mut().unwrap().kind = "loop_exit".into();
                for value in inputs {
                    self.edge(value, &merge, "loop_exit_input");
                }
                self.edge(condition.clone(), &merge, "control_dependency");
                for state in &exits {
                    for control in &state.controls {
                        self.edge(control.clone(), &merge, "control_dependency");
                    }
                }
                merge
            };
            self.locals.insert(name, value);
        }
        // A terminal else leaves only captured break paths able to continue.
        for state in exits {
            for control in state.controls {
                if (normal_terminal || control != condition) && !self.controls.contains(&control) {
                    self.controls.push(control);
                }
            }
        }
        Ok(false)
    }
    fn block(&mut self, body: Node, depth: usize) -> std::result::Result<bool, String> {
        if depth >= 32 {
            return Err("branch_depth_exhausted".into());
        }
        let mut returned = false;
        for statement in children(body).into_iter().filter(|n| n.kind() != "comment") {
            self.spend()?;
            if returned {
                return Err("statements_after_return_unverified".into());
            }
            match statement.kind() {
                "expression_statement" => {
                    let parts = children(statement);
                    if parts.len() != 1 {
                        return Err("expression_statement_shape_unverified".into());
                    }
                    let expression = parts[0];
                    if expression.kind() == "assignment" {
                        let left = expression
                            .child_by_field_name("left")
                            .ok_or("assignment_target_missing")?;
                        if left.kind() != "identifier"
                            || expression.child_by_field_name("type").is_some()
                        {
                            return Err("nonplain_assignment_unverified".into());
                        }
                        let right = expression
                            .child_by_field_name("right")
                            .ok_or("assignment_value_missing")?;
                        let input = self.expr(right, 0)?;
                        let name = text(left, self.source).to_string();
                        let definition = self.node(left, "definition", Some(name.clone()))?;
                        self.edge(input, &definition, "assignment");
                        self.locals.insert(binding_name(&name), definition);
                    } else if expression.kind() == "string"
                        && self.function.nodes.len() == self.function.parameters.len()
                        && !children(expression)
                            .iter()
                            .any(|n| n.kind() == "interpolation")
                    {
                        // A literal docstring has no value-flow effect.
                    } else {
                        let _ = self.expr(expression, 0)?;
                    }
                }
                "return_statement" => {
                    let values = children(statement);
                    if values.len() != 1 {
                        return Err("implicit_or_multiple_return_unverified".into());
                    }
                    let input = self.expr(values[0], 0)?;
                    let output = self.node(statement, "return", None)?;
                    self.edge(input, &output, "return_value");
                    let state = self.state()?;
                    self.return_states.push(ReturnState { output, state });
                    returned = true;
                }
                "try_statement" => {
                    returned = self.try_finally(statement, depth + 1)?;
                }
                "raise_statement" => {
                    let values = children(statement);
                    if statement.child_by_field_name("cause").is_some() || values.len() > 1 {
                        return Err("chained_raise_unverified".into());
                    }
                    let value = if let Some(value) = values.first() {
                        self.expr(*value, 0)?
                    } else {
                        self.active_exception
                            .last()
                            .cloned()
                            .ok_or("bare_raise_outside_captured_exception")?
                    };
                    let raised = self.node(statement, "raise", None)?;
                    self.edge(value, &raised, "exception_value");
                    let mut state = self.state()?;
                    state.controls.push(raised.clone());
                    state.exception_id = Some(raised);
                    self.propagate_exception(state)?;
                    returned = true;
                }
                "if_statement" => {
                    returned = self.branch(statement, depth + 1)?;
                }
                "while_statement" | "for_statement" => {
                    returned = self.loop_body(statement, depth + 1)?;
                }
                "break_statement" | "continue_statement" => {
                    self.jump(statement)?;
                    returned = true;
                }
                "pass_statement" => {}
                kind => return Err(format!("unsupported_statement:{kind}")),
            }
        }
        Ok(returned)
    }

    fn body(&mut self, node: Node) -> std::result::Result<(), String> {
        if node.parent().is_none_or(|p| p.kind() != "module")
            || node.child(0).is_some_and(|n| n.kind() == "async")
        {
            return Err("nested_decorated_method_or_async_function_unverified".into());
        }
        let params = node
            .child_by_field_name("parameters")
            .ok_or("parameters_missing")?;
        for p in children(params)
            .into_iter()
            .filter(|n| n.kind() != "comment")
        {
            if self.function.parameters.len() >= 256 {
                return Err("parameter_budget_exhausted".into());
            }
            if p.kind() != "identifier" {
                return Err("nonplain_parameters_unverified".into());
            }
            let name = text(p, self.source).to_string();
            if self.locals.contains_key(&binding_name(&name)) {
                return Err("duplicate_parameter".into());
            }
            let id = self.node(p, "parameter", Some(name.clone()))?;
            self.locals.insert(binding_name(&name), id.clone());
            self.function.parameters.push(id);
        }
        let body = node.child_by_field_name("body").ok_or("body_missing")?;
        let returned = self.block(body, 0)?;
        if !returned {
            return Err("implicit_return_unverified".into());
        }
        self.function.returns = self
            .return_states
            .iter()
            .map(|r| r.output.clone())
            .collect();
        Ok(())
    }
}
pub(super) fn capture(root: Node, source: &str, id: &str, symbols: &[Symbol]) -> Facts {
    let mut blocked = HashSet::new();
    let mut module_calls_qualified = true;
    for statement in children(root) {
        match statement.kind() {
            "function_definition" => {
                if statement.child_by_field_name("return_type").is_some()
                    || statement.child_by_field_name("parameters").is_none_or(|p| {
                        children(p)
                            .iter()
                            .any(|n| n.kind() != "identifier" && n.kind() != "comment")
                    })
                {
                    module_calls_qualified = false;
                }
            }
            "comment" => {}
            "import_statement" | "import_from_statement" => {
                if text(statement, source).contains('*') {
                    module_calls_qualified = false;
                }
            }
            "expression_statement" => {
                let parts = children(statement);
                if parts.len() == 1
                    && parts[0].kind() == "string"
                    && !children(parts[0])
                        .iter()
                        .any(|n| n.kind() == "interpolation")
                {
                    continue;
                }
                if parts.len() == 1
                    && matches!(parts[0].kind(), "assignment" | "augmented_assignment")
                {
                    if parts[0].child_by_field_name("right").is_none_or(|n| {
                        !matches!(
                            n.kind(),
                            "identifier"
                                | "integer"
                                | "float"
                                | "string"
                                | "true"
                                | "false"
                                | "none"
                        ) || n.kind() == "string"
                            && children(n).iter().any(|n| n.kind() == "interpolation")
                    }) {
                        module_calls_qualified = false;
                    }
                    if let Some(left) = parts[0]
                        .child_by_field_name("left")
                        .filter(|n| n.kind() == "identifier")
                    {
                        blocked.insert(binding_name(text(left, source)));
                    } else {
                        module_calls_qualified = false;
                    }
                } else {
                    module_calls_qualified = false;
                }
            }
            _ => module_calls_qualified = false,
        }
    }
    let mut facts = Facts {
        format: 7,
        complete: true,
        functions: Vec::new(),
    };
    let mut total_nodes = 0;
    let mut total_edges = 0;
    let mut stack = vec![root];
    let mut work = 100000;
    while let Some(node) = stack.pop() {
        if work == 0 || facts.functions.len() >= 512 {
            facts.complete = false;
            break;
        }
        work -= 1;
        if node.kind() == "function_definition" {
            if let Some(symbol) = symbols
                .iter()
                .find(|s| s.start_byte == node.start_byte() && s.kind == "function_definition")
            {
                let function = Function {
                    target_binding_qualified: module_calls_qualified
                        && !blocked.contains(&binding_name(&symbol.name)),
                    symbol: symbol.id.clone(),
                    qualified: true,
                    reason: None,
                    parameters: vec![],
                    returns: vec![],
                    nodes: vec![],
                    edges: vec![],
                    calls: vec![],
                };
                let mut builder = Builder {
                    source,
                    document: id,
                    function,
                    locals: HashMap::new(),
                    controls: Vec::new(),
                    loops: Vec::new(),
                    return_states: Vec::new(),
                    protected: Vec::new(),
                    flow_context: Vec::new(),
                    next_finally: 0,
                    active_exception: Vec::new(),
                    work: 16384,
                    blocked: &blocked,
                    module_calls_qualified,
                };
                let result = builder.body(node).and_then(|()| {
                    if builder.function.edges.len() > 8192 {
                        Err("capture_budget_exhausted".into())
                    } else {
                        Ok(())
                    }
                });
                let result = if total_nodes + builder.function.nodes.len() > 20000
                    || total_edges + builder.function.edges.len() > 40000
                {
                    Err("document_fact_budget_exhausted".into())
                } else {
                    result
                };
                if let Err(reason) = result {
                    if reason.contains("exhausted") {
                        facts.complete = false;
                    }
                    builder.function.qualified = false;
                    builder.function.reason = Some(reason);
                    builder.function.nodes.clear();
                    builder.function.edges.clear();
                    builder.function.calls.clear();
                    builder.function.parameters.clear();
                    builder.function.returns.clear();
                }
                total_nodes += builder.function.nodes.len();
                total_edges += builder.function.edges.len();
                let stop =
                    builder.function.reason.as_deref() == Some("document_fact_budget_exhausted");
                facts.functions.push(builder.function);
                if stop {
                    break;
                }
            }
        }
        stack.extend(children(node).into_iter().rev());
    }
    facts
}
