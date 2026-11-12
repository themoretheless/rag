//! Completion routing through conservative handlers, else and finally.
use super::*;
enum Completion {
    Normal,
    Return(String),
    Break(usize),
    Continue(usize),
    Exception,
}
impl Builder<'_> {
    pub(super) fn propagate_exception(
        &mut self,
        state: FlowState,
    ) -> std::result::Result<(), String> {
        if let Some(frame) = self.protected.last_mut() {
            if frame.len() >= 128 {
                return Err("exception_state_budget_exhausted".into());
            }
            frame.push(state);
        }
        Ok(())
    }
    pub(super) fn exception(
        &mut self,
        node: Node,
        event: &str,
        inputs: &[String],
    ) -> std::result::Result<(), String> {
        if event == "truth"
            && matches!(
                node.kind(),
                "integer" | "float" | "true" | "false" | "none" | "string"
            )
        {
            return Ok(());
        }
        if self.protected.is_empty() {
            return Ok(());
        }
        let trigger = self.node(node, &format!("exception_trigger_{event}"), None)?;
        for input in inputs {
            self.edge(input.clone(), &trigger, "exception_input");
        }
        let mut state = self.state()?;
        state.controls.push(trigger.clone());
        state.exception_id = Some(trigger);
        self.propagate_exception(state)
    }
    fn region(
        &mut self,
        body: Node,
        depth: usize,
    ) -> std::result::Result<Vec<(Completion, FlowState)>, String> {
        let returns_at = self.return_states.len();
        let jumps_at: Vec<_> = self
            .loops
            .iter()
            .map(|f| (f.breaks.len(), f.continues.len()))
            .collect();
        self.protected.push(Vec::new());
        let terminal = self.block(body, depth)?;
        let exceptions = self.protected.pop().unwrap();
        let mut incoming = Vec::new();
        if !terminal {
            incoming.push((Completion::Normal, self.state()?));
        }
        for r in self.return_states.split_off(returns_at) {
            incoming.push((Completion::Return(r.output), r.state));
        }
        for (index, (b, c)) in jumps_at.into_iter().enumerate() {
            for state in self.loops[index].breaks.split_off(b) {
                incoming.push((Completion::Break(index), state));
            }
            for state in self.loops[index].continues.split_off(c) {
                incoming.push((Completion::Continue(index), state));
            }
        }
        incoming.extend(exceptions.into_iter().map(|s| (Completion::Exception, s)));
        if incoming.len() > 128 {
            return Err("completion_state_budget_exhausted".into());
        }
        Ok(incoming)
    }
    fn restore(&mut self, state: &FlowState, context: Vec<String>) {
        self.locals = state.locals.clone();
        self.controls = state.controls.clone();
        self.flow_context = context;
    }
    fn filter_shape(&mut self, node: Node, depth: usize) -> std::result::Result<(), String> {
        self.spend()?;
        if depth >= 16 {
            return Err("exception_filter_depth_exhausted".into());
        }
        match node.kind() {
            "identifier" => Ok(()),
            "attribute" | "tuple" | "parenthesized_expression" => {
                for child in children(node).into_iter().filter(|n| n.kind() != "comment") {
                    self.filter_shape(child, depth + 1)?;
                }
                Ok(())
            }
            _ => Err("dynamic_exception_filter_unverified".into()),
        }
    }
    fn selector(
        &mut self,
        node: Node,
        filter: &str,
        depth: usize,
    ) -> std::result::Result<(), String> {
        self.spend()?;
        if depth >= 16 {
            return Err("exception_filter_depth_exhausted".into());
        }
        match node.kind() {
            "identifier" => {
                if let Some(value) = self
                    .locals
                    .get(&binding_name(text(node, self.source)))
                    .cloned()
                {
                    self.edge(value, filter, "exception_filter_input");
                }
                Ok(())
            }
            "attribute" => self.selector(
                node.child_by_field_name("object")
                    .ok_or("invalid_exception_filter_attribute")?,
                filter,
                depth + 1,
            ),
            "tuple" | "parenthesized_expression" => {
                for child in children(node).into_iter().filter(|n| n.kind() != "comment") {
                    self.selector(child, filter, depth + 1)?;
                }
                Ok(())
            }
            _ => Err("dynamic_exception_filter_unverified".into()),
        }
    }
    fn emit(
        &mut self,
        completion: Completion,
        state: FlowState,
        normals: &mut Vec<FlowState>,
    ) -> std::result::Result<(), String> {
        match completion {
            Completion::Normal => normals.push(state),
            Completion::Return(output) => {
                for control in &state.controls {
                    self.edge(control.clone(), &output, "completion_control");
                }
                self.return_states.push(ReturnState { output, state });
            }
            Completion::Break(index) | Completion::Continue(index) => {
                let frame = &mut self.loops[index];
                if frame.breaks.len() + frame.continues.len() >= 128 {
                    return Err("loop_state_budget_exhausted".into());
                }
                if matches!(completion, Completion::Break(_)) {
                    frame.breaks.push(state);
                } else {
                    frame.continues.push(state);
                }
            }
            Completion::Exception => self.propagate_exception(state)?,
        }
        Ok(())
    }
    pub(super) fn try_finally(
        &mut self,
        node: Node,
        depth: usize,
    ) -> std::result::Result<bool, String> {
        if depth >= 32 || self.protected.len() >= 32 || self.flow_context.len() >= 32 {
            return Err("exception_region_depth_exhausted".into());
        }
        let clauses = children(node);
        let handlers: Vec<_> = clauses
            .iter()
            .copied()
            .filter(|n| n.kind() == "except_clause")
            .collect();
        let mut seen_bare = false;
        for clause in &handlers {
            if seen_bare {
                return Err("handler_after_bare_except_unverified".into());
            }
            let (types, alias) = python_names::handler_parts(*clause)?;
            seen_bare = types.is_empty();
            for ty in types {
                self.filter_shape(ty, 0)?;
            }
            let _ = alias;
            let mut cursor = clause.walk();
            if clause.children(&mut cursor).any(|n| n.kind() == "*") {
                return Err("exception_groups_unverified".into());
            }
        }
        let final_body = clauses
            .iter()
            .find(|n| n.kind() == "finally_clause")
            .and_then(|n| children(*n).into_iter().find(|n| n.kind() == "block"));
        let else_body = clauses
            .iter()
            .find(|n| n.kind() == "else_clause")
            .and_then(|n| n.child_by_field_name("body"));
        let body = node.child_by_field_name("body").ok_or("try_body_missing")?;
        let base = self.flow_context.clone();
        let outer = self.controls.clone();
        let serial = self.next_finally;
        self.next_finally += 1;
        let incoming = self.region(body, depth)?;
        let mut routed = Vec::new();
        for (index, (completion, state)) in incoming.into_iter().enumerate() {
            self.spend()?;
            match completion {
                Completion::Normal if else_body.is_some() => {
                    let mut context = base.clone();
                    context.push(format!("else:{serial}:{index}"));
                    self.restore(&state, context);
                    routed.extend(self.region(else_body.unwrap(), depth)?);
                }
                Completion::Exception => {
                    let mut catch_all = false;
                    for (handler_index, clause) in handlers.iter().enumerate() {
                        self.spend()?;
                        let mut context = base.clone();
                        context.push(format!("handler:{serial}:{index}:{handler_index}"));
                        self.restore(&state, context);
                        let exception = state
                            .exception_id
                            .as_ref()
                            .ok_or("exception_value_missing")?;
                        let filter = self.node(*clause, "exception_match", None)?;
                        self.edge(exception.clone(), &filter, "exception_match_candidate");
                        let (types, alias) = python_names::handler_parts(*clause)?;
                        catch_all = types.is_empty();
                        for ty in types {
                            self.selector(ty, &filter, 0)?;
                        }
                        self.controls.push(filter);
                        let value = self.node(*clause, "caught_exception", None)?;
                        self.edge(exception.clone(), &value, "exception_object_candidate");
                        let alias = if let Some(alias) = alias {
                            let name = text(alias, self.source).to_string();
                            let definition = self.node(alias, "definition", Some(name.clone()))?;
                            self.edge(value.clone(), &definition, "exception_binding");
                            let name = binding_name(&name);
                            self.locals.insert(name.clone(), definition);
                            Some(name)
                        } else {
                            None
                        };
                        let handler_body = children(*clause)
                            .into_iter()
                            .find(|n| n.kind() == "block")
                            .ok_or("handler_body_missing")?;
                        self.active_exception.push(value);
                        let mut results = self.region(handler_body, depth)?;
                        self.active_exception.pop();
                        if let Some(alias) = alias {
                            for (_, state) in &mut results {
                                state.locals.remove(&alias);
                            }
                        }
                        routed.extend(results);
                        if routed.len() > 128 {
                            return Err("handler_route_budget_exhausted".into());
                        }
                        if catch_all {
                            break;
                        }
                    }
                    if !catch_all {
                        routed.push((Completion::Exception, state));
                    }
                }
                _ => routed.push((completion, state)),
            }
            if routed.len() > 128 {
                return Err("handler_route_budget_exhausted".into());
            }
        }
        let mut normals = Vec::new();
        let mut terminal_routes = false;
        if let Some(final_body) = final_body {
            let names: BTreeSet<_> = routed
                .iter()
                .flat_map(|(_, s)| s.locals.keys().cloned())
                .collect();
            let mut varying = BTreeSet::new();
            for name in names {
                self.spend()?;
                let values: BTreeSet<_> = routed.iter().map(|(_, s)| s.locals.get(&name)).collect();
                if values.len() > 1 {
                    varying.insert(name);
                }
            }
            for (index, (completion, state)) in routed.into_iter().enumerate() {
                self.spend()?;
                let mut context = base.clone();
                context.push(format!("finally:{serial}:{index}"));
                self.restore(&state, context);
                self.controls = outer.clone();
                for name in &varying {
                    self.spend()?;
                    if let Some(value) = state.locals.get(name) {
                        let proxy = self.node(node, &format!("completion_binding:{name}"), None)?;
                        self.function.nodes.last_mut().unwrap().kind = "completion_binding".into();
                        self.edge(value.clone(), &proxy, "completion_value");
                        for control in &state.controls {
                            if !outer.contains(control) {
                                self.edge(control.clone(), &proxy, "completion_control");
                            }
                        }
                        self.locals.insert(name.clone(), proxy);
                    }
                }
                if matches!(completion, Completion::Exception) {
                    self.active_exception.push(
                        state
                            .exception_id
                            .clone()
                            .ok_or("exception_value_missing")?,
                    );
                }
                let results = self.region(final_body, depth)?;
                if matches!(completion, Completion::Exception) {
                    self.active_exception.pop();
                }
                // A region has at most one merged normal continuation.
                let mut pending = Some(completion);
                for (outcome, mut updated) in results {
                    let outcome = if matches!(outcome, Completion::Normal) {
                        for control in &state.controls {
                            if !updated.controls.contains(control) {
                                updated.controls.push(control.clone());
                            }
                        }
                        let original = pending
                            .take()
                            .ok_or("duplicate_normal_finally_continuation")?;
                        if matches!(original, Completion::Exception) {
                            updated.exception_id = state.exception_id.clone();
                        }
                        original
                    } else {
                        outcome
                    };
                    terminal_routes |= !matches!(outcome, Completion::Normal);
                    self.emit(outcome, updated, &mut normals)?;
                }
            }
        } else {
            for (completion, state) in routed {
                terminal_routes |= !matches!(completion, Completion::Normal);
                self.emit(completion, state, &mut normals)?;
            }
        }
        self.flow_context = base;
        self.controls = outer;
        if normals.is_empty() {
            return Ok(true);
        }
        let mut names: Vec<_> = normals[0].locals.keys().cloned().collect();
        names.sort();
        self.locals.clear();
        for name in names {
            self.spend()?;
            if normals.iter().any(|s| !s.locals.contains_key(&name)) {
                continue;
            }
            let inputs: BTreeSet<_> = normals.iter().map(|s| s.locals[&name].clone()).collect();
            let value = if inputs.len() == 1 {
                inputs.into_iter().next().unwrap()
            } else {
                let merge = self.node(node, &format!("try_merge:{name}"), None)?;
                self.function.nodes.last_mut().unwrap().kind = "try_merge".into();
                for input in inputs {
                    self.edge(input, &merge, "try_merge_input");
                }
                for state in &normals {
                    for control in &state.controls {
                        self.edge(control.clone(), &merge, "completion_control");
                    }
                }
                merge
            };
            self.locals.insert(name, value);
        }
        if terminal_routes {
            for state in normals {
                for control in state.controls {
                    if !self.controls.contains(&control) {
                        self.controls.push(control);
                    }
                }
            }
        }
        Ok(false)
    }
}
