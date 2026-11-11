//! Bash/POSIX-sh source candidates. Does not execute scripts or perform PATH/filesystem searches.
use super::*;
use std::path::{Component, Path, PathBuf};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    functions: Vec<Function>,
    sites: Vec<Site>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Function {
    id: String,
    name: String,
    start: usize,
    end: usize,
    owner: Option<String>,
    conditional: bool,
    boundaries: Vec<(usize, usize)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    start: usize,
    line: usize,
    name: Option<String>,
    args: Vec<Option<String>>,
    owner: Option<String>,
    conditional: bool,
    boundaries: Vec<(usize, usize)>,
    kind: String,
}
fn diag(out: &mut IndexedDocument, code: &str) {
    if !out.binding_limitations.iter().any(|c| c == code) {
        out.binding_limitations.push(code.into());
    }
}
pub(super) fn literal(node: Node, source: &str) -> Option<String> {
    let raw = text(node, source);
    match node.kind() {
        "word" | "variable_name"
            if !raw.contains(['$', '`', '\\', '*', '?', '[', ']', '{', '}', '~']) =>
        {
            Some(raw.into())
        }
        "raw_string" => raw
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .map(str::to_owned),
        "string" if !raw.contains(['$', '`', '\\']) => raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(str::to_owned),
        "command_name" => node.named_child(0).and_then(|n| literal(n, source)),
        _ => None,
    }
}
fn owner(node: Node, source: &str, doc: &str) -> Option<String> {
    let mut p = node.parent();
    while let Some(n) = p {
        if n.kind() == "function_definition" {
            let name = n.child_by_field_name("name")?;
            return Some(format!("{}:{}:{}", doc, n.start_byte(), text(name, source)));
        }
        p = n.parent();
    }
    None
}
fn context(node: Node) -> (bool, Vec<(usize, usize)>) {
    let mut conditional = false;
    let mut boundaries = vec![];
    let mut p = node.parent();
    while let Some(n) = p {
        if matches!(
            n.kind(),
            "if_statement"
                | "elif_clause"
                | "else_clause"
                | "case_statement"
                | "case_item"
                | "while_statement"
                | "for_statement"
                | "c_style_for_statement"
                | "list"
        ) {
            conditional = true;
        }
        if matches!(
            n.kind(),
            "subshell" | "pipeline" | "command_substitution" | "process_substitution"
        ) {
            boundaries.push((n.start_byte(), n.end_byte()));
        }
        p = n.parent();
    }
    (conditional, boundaries)
}
fn args(node: Node, source: &str) -> Vec<Option<String>> {
    let mut c = node.walk();
    node.children_by_field_name("argument", &mut c)
        .map(|n| literal(n, source))
        .collect()
}
fn named_words(node: Node, source: &str) -> Vec<Option<String>> {
    let mut c = node.walk();
    node.named_children(&mut c)
        .map(|n| literal(n, source))
        .collect()
}
fn site(node: Node, source: &str, doc: &str) -> Option<Site> {
    let (name, args, kind) = match node.kind() {
        "command" => (
            node.child_by_field_name("name")
                .and_then(|n| literal(n, source)),
            args(node, source),
            "command",
        ),
        "unset_command" => (Some("unset".into()), named_words(node, source), "command"),
        "declaration_command" if text(node, source).trim_start().starts_with("export ") => {
            (Some("export".into()), named_words(node, source), "export")
        }
        _ => return None,
    };
    let (conditional, boundaries) = context(node);
    Some(Site {
        start: node.start_byte(),
        line: node.start_position().row + 1,
        name,
        args,
        owner: owner(node, source, doc),
        conditional,
        boundaries,
        kind: kind.into(),
    })
}
pub(super) fn collect(
    node: Node,
    source: &str,
    doc: &str,
    out: &mut IndexedDocument,
) -> Option<String> {
    if node.kind() == "program" {
        diag(out, "SHELL_ENVIRONMENT_AND_EXECUTION_UNVERIFIED");
    }
    if let Some(site) = site(node, source, doc) {
        if site.name.is_none() {
            diag(out, "SHELL_DYNAMIC_COMMAND_UNRESOLVED");
        }
        if matches!(
            site.name.as_deref(),
            Some("eval" | "alias" | "unalias" | "trap")
        ) {
            diag(out, "SHELL_DYNAMIC_NAMESPACE_EFFECTS_UNVERIFIED");
        }
        if matches!(site.name.as_deref(), Some("source" | ".")) && source_path(&site).is_none() {
            diag(out, "SHELL_SOURCE_CWD_OR_EXPANSION_UNVERIFIED");
        }
        if !site.boundaries.is_empty() {
            diag(out, "SHELL_SUBPROCESS_ENVIRONMENT_UNVERIFIED");
        }
        out.shell.as_mut().unwrap().sites.push(site);
    }
    if node.kind() != "function_definition" {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body")?;
    let raw = text(name, source).to_owned();
    let id = format!("{}:{}:{}", doc, node.start_byte(), raw);
    let (conditional, boundaries) = context(node);
    let parent = owner(node, source, doc);
    if parent.is_some() {
        diag(out, "SHELL_NESTED_FUNCTION_REGISTRATION_UNVERIFIED");
    }
    out.symbols.push(Symbol {
        build_context: None,
        documentation: None,
        documentation_examined: false,
        parameter_contracts: None,
        id: id.clone(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: raw.clone(),
        qualified_name: raw.clone(),
        kind: "function_definition".into(),
        signature: source[node.start_byte()..body.start_byte()].trim().into(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        revision: out.revision,
        content_hash: out.hash.clone(),
    });
    out.shell.as_mut().unwrap().functions.push(Function {
        id: id.clone(),
        name: raw,
        start: node.start_byte(),
        end: node.end_byte(),
        owner: parent,
        conditional,
        boundaries,
    });
    Some(id)
}
fn exported(site: &Site) -> Vec<&str> {
    if site.kind != "export" || !site.args.iter().any(|s| s.as_deref() == Some("-f")) {
        return vec![];
    }
    site.args
        .iter()
        .filter_map(|s| s.as_deref())
        .filter(|s| !s.starts_with('-'))
        .collect()
}
pub(super) fn push_references(
    node: Node,
    source: &str,
    doc: &str,
    out: &mut IndexedDocument,
    caller: Option<&str>,
) {
    let Some(site) = site(node, source, doc) else {
        return;
    };
    let mut names = if site.kind == "export" {
        exported(&site)
            .into_iter()
            .map(|s| (s.to_owned(), false))
            .collect::<Vec<_>>()
    } else {
        vec![(
            site.name
                .clone()
                .unwrap_or_else(|| "<dynamic-command>".into()),
            true,
        )]
    };
    if site.kind == "export" {
        names.push(("export".into(), true));
    }
    for (name, is_call) in names {
        let name_node = node.child_by_field_name("name").unwrap_or(node);
        let expression = if is_call {
            text(name_node, source).to_owned()
        } else {
            format!("export -f {name}")
        };
        out.references.push(Reference {
            build_context: None,
            start_byte: node.start_byte(),
            document_id: doc.into(),
            path: out.path.clone(),
            name,
            expression,
            line: name_node.start_position().row + 1,
            column: name_node.start_position().column + 1,
            caller: caller.map(str::to_owned),
            is_call,
            revision: out.revision,
            content_hash: out.hash.clone(),
        });
    }
}
fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}
fn source_path(site: &Site) -> Option<String> {
    let path = site
        .args
        .iter()
        .find(|s| s.as_deref() != Some("--"))?
        .as_deref()?;
    if !Path::new(path).is_absolute() || path.contains('\0') {
        return None;
    }
    Some(normalized(Path::new(path)).to_string_lossy().into_owned())
}
fn source_doc<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    site: &Site,
) -> Option<&'a IndexedDocument> {
    let path = source_path(site)?;
    let mut found = docs.values().filter(|d| {
        d.path == path && d.project == doc.project && d.shell.is_some() && !d.parse_errors
    });
    let first = found.next()?;
    if found.next().is_some() {
        None
    } else {
        Some(first)
    }
}
#[derive(Clone)]
struct Candidate<'a> {
    doc: &'a IndexedDocument,
    function: &'a Function,
    conditional: bool,
    available: usize,
}
#[derive(Default)]
struct Env<'a> {
    values: HashMap<String, Vec<Candidate<'a>>>,
    history: HashMap<String, Vec<Candidate<'a>>>,
    unknown: bool,
    aliases: HashSet<String>,
    mutated: HashSet<String>,
    loaded: Vec<&'a IndexedDocument>,
    local: HashSet<String>,
}
fn boundaries_allowed(bounds: &[(usize, usize)], at: usize, foreign: bool) -> bool {
    if foreign {
        bounds.is_empty()
    } else {
        bounds.iter().all(|(s, e)| *s <= at && at < *e)
    }
}
fn register<'a>(env: &mut Env<'a>, candidate: Candidate<'a>, local: bool) {
    let name = candidate.function.name.clone();
    env.history
        .entry(name.clone())
        .or_default()
        .push(candidate.clone());
    if candidate.conditional {
        env.values.entry(name.clone()).or_default().push(candidate);
    } else {
        env.values.insert(name.clone(), vec![candidate]);
        if local {
            env.local.insert(name);
        }
    }
}
fn mutation<'a>(env: &mut Env<'a>, doc: &'a IndexedDocument, site: &Site) -> bool {
    let Some(name) = site.name.as_deref() else {
        env.unknown = true;
        return true;
    };
    if let Some(functions) = env.values.get(name).cloned() {
        for candidate in functions {
            let facts = candidate.doc.shell.as_ref().unwrap();
            for nested in facts
                .functions
                .iter()
                .filter(|f| f.owner.as_deref() == Some(&candidate.function.id))
            {
                register(
                    env,
                    Candidate {
                        doc: candidate.doc,
                        function: nested,
                        conditional: true,
                        available: site.start,
                    },
                    false,
                );
            }
            if facts
                .sites
                .iter()
                .filter(|s| s.owner.as_deref() == Some(&candidate.function.id))
                .any(|s| {
                    matches!(
                        s.name.as_deref(),
                        Some("eval" | "trap" | "source" | "." | "alias" | "unset")
                    )
                })
            {
                env.unknown = true;
            }
        }
        return true;
    }
    if name == "unset" {
        let function_only = site.args.iter().any(|s| s.as_deref() == Some("-f"));
        for arg in &site.args {
            let Some(arg) = arg else {
                env.unknown = true;
                continue;
            };
            if arg.starts_with('-') {
                continue;
            }
            env.mutated.insert(arg.clone());
            if function_only && !site.conditional {
                env.values.remove(arg);
            } else {
                env.aliases.insert(arg.clone());
            }
        }
        return true;
    }
    if name == "alias" {
        for arg in &site.args {
            if let Some((name, _)) = arg.as_deref().and_then(|a| a.split_once('=')) {
                env.aliases.insert(name.into());
                env.mutated.insert(name.into());
            } else {
                env.unknown = true;
            }
        }
        return true;
    }
    if name == "unalias" {
        for arg in &site.args {
            if let Some(name) = arg {
                if name.starts_with('-') {
                    env.unknown = true;
                } else if !site.conditional {
                    env.aliases.remove(name);
                }
            } else {
                env.unknown = true;
            }
        }
        return true;
    }
    if matches!(name, "eval" | "trap") {
        env.unknown = true;
        return true;
    }
    let _ = doc;
    false
}
#[derive(Clone, Copy)]
enum Event<'a> {
    Function(&'a Function),
    Site(&'a Site),
}
#[derive(Clone, Copy)]
struct ReplayFrame<'a> {
    owner: Option<&'a str>,
    until: usize,
    query_at: usize,
    foreign: bool,
    available: Option<usize>,
    inherited_conditional: bool,
}
fn replay<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &'a IndexedDocument,
    frame: ReplayFrame<'_>,
    env: &mut Env<'a>,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) {
    let ReplayFrame {
        owner,
        until,
        query_at,
        foreign,
        available,
        inherited_conditional,
    } = frame;
    if *budget == 0 || !stack.insert(format!("{}:{owner:?}", doc.path)) {
        env.unknown = true;
        return;
    }
    *budget -= 1;
    if !env.loaded.iter().any(|d| std::ptr::eq(*d, doc)) {
        env.loaded.push(doc);
    }
    let facts = doc.shell.as_ref().unwrap();
    let mut events = vec![];
    for f in &facts.functions {
        if f.owner.as_deref() == owner
            && f.start < until
            && boundaries_allowed(&f.boundaries, query_at, foreign)
        {
            events.push((f.start, Event::Function(f)));
        }
    }
    for s in &facts.sites {
        if s.owner.as_deref() == owner
            && s.start < until
            && boundaries_allowed(&s.boundaries, query_at, foreign)
        {
            events.push((s.start, Event::Site(s)));
        }
    }
    events.sort_by_key(|e| e.0);
    for (_, event) in events {
        match event {
            Event::Function(function) => register(
                env,
                Candidate {
                    doc,
                    function,
                    conditional: function.conditional || inherited_conditional,
                    available: available.unwrap_or(function.start),
                },
                owner.is_some() && !foreign,
            ),
            Event::Site(site) => {
                let head = site.name.as_deref();
                if matches!(head, Some("source" | ".")) && !env.values.contains_key(head.unwrap()) {
                    if let Some(target) = source_doc(docs, doc, site) {
                        replay(
                            docs,
                            target,
                            ReplayFrame {
                                owner: None,
                                until: usize::MAX,
                                query_at: 0,
                                foreign: true,
                                available: Some(available.unwrap_or(site.start)),
                                inherited_conditional: inherited_conditional || site.conditional,
                            },
                            env,
                            stack,
                            budget,
                        );
                    } else {
                        env.unknown = true;
                    }
                } else {
                    mutation(env, doc, site);
                }
            }
        }
    }
    stack.remove(&format!("{}:{owner:?}", doc.path));
}
pub(super) fn resolve(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    r: &Reference,
) -> CallEdge {
    let mut found = vec![];
    let mut complete = true;
    let mut reason = "shell_not_registered_in_captured_context";
    let Some(current) = docs.get(&r.document_id) else {
        return CallEdge {
            reference: r.clone(),
            candidates: vec![],
            resolution: "unresolved".into(),
            provenance: "derived",
            resolution_basis: "shell_document_unavailable".into(),
        };
    };
    let site = current
        .shell
        .as_ref()
        .unwrap()
        .sites
        .iter()
        .find(|s| s.start == r.start_byte);
    let mut env = Env::default();
    if !doc.parse_errors {
        if let Some(owner) = r.caller.as_deref() {
            replay(
                docs,
                current,
                ReplayFrame {
                    owner: None,
                    until: usize::MAX,
                    query_at: r.start_byte,
                    foreign: false,
                    available: None,
                    inherited_conditional: false,
                },
                &mut env,
                &mut HashSet::new(),
                &mut 128,
            );
            // The body has no proven invocation time; retain all observed source registrations.
            env.values = env.history.clone();
            env.local.clear();
            replay(
                docs,
                current,
                ReplayFrame {
                    owner: Some(owner),
                    until: r.start_byte,
                    query_at: r.start_byte,
                    foreign: false,
                    available: None,
                    inherited_conditional: false,
                },
                &mut env,
                &mut HashSet::new(),
                &mut 128,
            );
        } else {
            replay(
                docs,
                current,
                ReplayFrame {
                    owner: None,
                    until: r.start_byte,
                    query_at: r.start_byte,
                    foreign: false,
                    available: None,
                    inherited_conditional: false,
                },
                &mut env,
                &mut HashSet::new(),
                &mut 128,
            );
        }
        if let Some(site) = site {
            let target = if site.kind == "export" {
                Some(r.name.as_str())
            } else {
                site.name.as_deref()
            };
            if let Some(name) = target {
                if !name.contains('/')
                    && !env.aliases.contains(name)
                    && !(site.kind == "export" && !r.is_call && env.values.contains_key("export"))
                {
                    let candidates = env.values.get(name).cloned().unwrap_or_default();
                    let owner_start = r
                        .caller
                        .as_ref()
                        .and_then(|id| {
                            current
                                .shell
                                .as_ref()
                                .unwrap()
                                .functions
                                .iter()
                                .find(|f| &f.id == id)
                        })
                        .map(|f| f.start);
                    for candidate in candidates {
                        if !symbols.iter().any(|s| s.id == candidate.function.id) {
                            continue;
                        }
                        complete &= !candidate.conditional;
                        if let Some(start) = owner_start {
                            if !env.local.contains(name) && candidate.available > start {
                                complete = false;
                            }
                        }
                        reason = if std::ptr::eq(candidate.doc, current) {
                            "shell_local_function"
                        } else {
                            "shell_sourced_function"
                        };
                        found.push(candidate.function.id.clone());
                    }
                    if found.is_empty() {
                        reason = match name {
                            "source" | "." => "shell_source_directive",
                            "command" | "builtin" | "env" | "exec" | "sudo" => {
                                "shell_command_bypasses_function_argument"
                            }
                            _ => "shell_external_or_unregistered_command",
                        };
                    }
                    complete &= !env.unknown;
                    if r.caller.is_some() && env.mutated.contains(name) {
                        complete = false;
                    }
                } else {
                    reason = "shell_alias_or_explicit_executable_path";
                }
            } else {
                reason = "shell_dynamic_command";
            }
        }
    } else {
        reason = "shell_parse_unverified";
    }
    found.sort();
    found.dedup();
    CallEdge {
        reference: r.clone(),
        resolution: match found.len() {
            0 => "unresolved",
            1 if complete => "resolved_static",
            _ => "ambiguous",
        }
        .into(),
        candidates: found,
        provenance: "derived",
        resolution_basis: if !complete {
            "shell_registration_or_execution_context_unverified".into()
        } else {
            reason.into()
        },
    }
}
pub(super) fn diagnostics(docs: &HashMap<String, IndexedDocument>) -> Vec<serde_json::Value> {
    let mut out = vec![];
    for (id, doc) in docs {
        if let Some(facts) = &doc.shell {
            for site in &facts.sites {
                if matches!(site.name.as_deref(), Some("source" | ".")) {
                    let path = source_path(site);
                    let target = source_doc(docs, doc, site);
                    out.push(serde_json::json!({"document_id":id,"path":doc.path,"line":site.line,"code":"SHELL_SOURCE_DEPENDENCY","target_path":path,"captured":target.is_some(),"relative_cwd_or_dynamic":source_path(site).is_none(),"execution_proven":false}));
                }
            }
        }
    }
    out.sort_by_key(serde_json::Value::to_string);
    out
}
pub(super) fn entrypoint(_doc: &IndexedDocument, s: &Symbol) -> Option<&'static str> {
    (s.name == "main").then_some("shell_main_candidate")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut out = CodeSnapshot::default();
        for (i, (path, project, source)) in files.iter().enumerate() {
            let mut d = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!d.parse_errors, "{path}");
            d.project = Some((*project).into());
            out.symbols.extend(d.symbols.clone());
            out.references.extend(d.references.clone());
            docs.insert(i.to_string(), d);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|r| r.reference.is_call)
            .cloned()
            .collect();
        out.sql = sql::snapshot(&docs);
        out
    }
    fn calls<'a>(s: &'a CodeSnapshot, name: &str) -> Vec<&'a CallEdge> {
        let mut result: Vec<_> = s
            .calls
            .iter()
            .filter(|c| c.reference.name == name)
            .collect();
        result.sort_by_key(|c| c.reference.start_byte);
        result
    }
    #[test]
    fn registration_order_redefinition_and_unset() {
        let s = snapshot(&[(
            "/p/main.sh",
            "p",
            "work\nwork(){ :; }\nwork\nwork(){ true; }\nwork\nunset -f work\nwork\n",
        )]);
        let c = calls(&s, "work");
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].resolution, "unresolved");
        assert_eq!(c[1].resolution, "resolved_static");
        assert_eq!(c[2].resolution, "resolved_static");
        assert_ne!(c[1].candidates, c[2].candidates);
        assert_eq!(c[3].resolution, "unresolved");
    }
    #[test]
    fn captured_source_project_and_cwd_boundaries() {
        for (path, project, expected) in [
            ("/p/lib.sh", "p", "resolved_static"),
            ("./lib.sh", "p", "unresolved"),
            ("/p/lib.sh", "other", "unresolved"),
        ] {
            let main = format!("source '{path}'\nwork\n");
            let s = snapshot(&[
                ("/p/lib.sh", project, "work(){ :; }"),
                ("/p/main.bash", "p", &main),
            ]);
            assert_eq!(
                calls(&s, "work")[0].resolution,
                expected,
                "{path}/{project}"
            );
        }
    }
    #[test]
    fn subshell_definitions_do_not_escape_and_dynamic_calls_stay_unknown() {
        let s = snapshot(&[("/p/main.sh", "p", "( work(){ :; }; work )\nwork\n\"$fn\"\n")]);
        let c = calls(&s, "work");
        assert_eq!(c[0].resolution, "resolved_static");
        assert_eq!(c[1].resolution, "unresolved");
        assert_eq!(calls(&s, "<dynamic-command>")[0].resolution, "unresolved");
    }
    #[test]
    fn overridden_builtin_and_export_function_values() {
        let s = snapshot(&[(
            "/p/main.sh",
            "p",
            "work(){ :; }\neval(){ :; }\neval true\nwork\nexport -f work\n",
        )]);
        assert_eq!(calls(&s, "work")[0].resolution, "resolved_static");
        assert_eq!(calls(&s, "eval")[0].resolution, "resolved_static");
        assert!(s.bound_references.iter().any(|c| !c.reference.is_call
            && c.reference.name == "work"
            && c.resolution == "resolved_static"));
        assert_eq!(calls(&s, "export").len(), 1);
    }
    #[test]
    fn sql_literals_do_not_apply_json_or_shell_expansion() {
        let s = snapshot(&[("/p/main.sh", "p", "query 'SELECT * FROM users'\nquery \"UPDATE users SET name = 'Ada'\"\nquery \"SELECT * FROM $table\"\n")]);
        assert!(s
            .sql
            .usages
            .iter()
            .any(|u| u.table == "users" && u.mode == "read"));
        assert!(s
            .sql
            .usages
            .iter()
            .any(|u| u.table == "users" && u.mode == "write"));
        assert!(!s.sql.usages.iter().any(|u| u.table.contains("table")));
        assert!(s
            .sql
            .diagnostics
            .iter()
            .any(|d| d["code"] == "SHELL_SQL_VALUE_UNVERIFIED"));
    }
}
