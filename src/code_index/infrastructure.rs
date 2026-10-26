//! Captured deployment/configuration facts, not observed execution or connectivity.
use super::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};
mod docker;
#[cfg(test)]
mod tests;
mod yaml;
use yaml::{Data, Yaml};
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InfrastructureQuery {
    pub project: String,
    /// Explicit additional project identities; exact image-name candidate links only.
    pub linked_projects: Option<Vec<String>>,
    /// Frozen revision; omitted means latest with current stored-input guards.
    pub index_revision: Option<i64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub max_depth: Option<usize>,
    pub max_paths: Option<usize>,
    pub max_work: Option<usize>,
    pub limit: Option<usize>,
}
#[derive(Clone, Serialize)]
struct Evidence {
    document_id: String,
    path: String,
    project: String,
    revision: i64,
    content_hash: String,
    start_byte: usize,
    end_byte: usize,
    line: usize,
}
#[derive(Clone, Serialize)]
struct InfraNode {
    id: String,
    kind: String,
    name: String,
    project: String,
    evidence: Evidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol_id: Option<String>,
}
#[derive(Clone, Serialize)]
struct Edge {
    source: String,
    target: String,
    relation: String,
    evidence: Evidence,
    resolution: &'static str,
    runtime_proven: bool,
    supporting_evidence: Vec<Evidence>,
}
struct Graph {
    nodes: BTreeMap<String, InfraNode>,
    edges: Vec<Edge>,
    diagnostics: Vec<Value>,
    files: Vec<history::File>,
    code: HashMap<String, IndexedDocument>,
}
type SelectorRecord = (String, String, BTreeMap<String, String>, Evidence);
fn evidence(file: &history::File, start: usize, end: usize, line: usize) -> Evidence {
    Evidence {
        document_id: file.id.clone(),
        path: file.path.clone(),
        project: file.project.clone().unwrap_or_default(),
        revision: file.revision,
        content_hash: file.hash.clone(),
        start_byte: start,
        end_byte: end,
        line,
    }
}
fn anchor(file: &history::File, n: &Yaml) -> Evidence {
    evidence(file, n.start, n.end, n.line)
}
fn literal(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && !s.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '$' | '{' | '}' | '`' | '\\' | '\'' | '"' | ';' | '|' | '&' | '<' | '>'
                )
        })
}
fn name(s: &str) -> bool {
    literal(s)
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}
fn image(s: &str) -> bool {
    literal(s)
        && !s.contains("://")
        && !s.chars().any(char::is_whitespace)
        && (!s.contains('@')
            || s.split_once("@sha256:").is_some_and(|(a, b)| {
                !a.contains('@') && b.len() == 64 && b.bytes().all(|c| c.is_ascii_hexdigit())
            }))
}
fn normalized(path: &Path) -> Option<String> {
    let mut p = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => (),
            Component::ParentDir => {
                if !p.pop() {
                    return None;
                }
            }
            other => p.push(other.as_os_str()),
        }
    }
    Some(p.to_string_lossy().into_owned())
}
impl Graph {
    fn node(&mut self, kind: &str, name: &str, scope: &str, e: Evidence) -> String {
        let id = format!(
            "infra:{}",
            content_hash(&serde_json::to_string(&(kind, name, scope, &e.project)).unwrap())
        );
        self.nodes.entry(id.clone()).or_insert_with(|| InfraNode {
            id: id.clone(),
            kind: kind.into(),
            name: name.into(),
            project: e.project.clone(),
            evidence: e,
            symbol_id: None,
        });
        id
    }
    fn edge(&mut self, source: &str, target: &str, relation: &str, e: Evidence) {
        self.edges.push(Edge {
            source: source.into(),
            target: target.into(),
            relation: relation.into(),
            evidence: e,
            resolution: "captured_static_candidate",
            runtime_proven: false,
            supporting_evidence: Vec::new(),
        });
    }
    fn diagnostic(&mut self, file: &history::File, code: &str) {
        self.diagnostics.push(
            json!({"document_id":file.id,"path":file.path,"project":file.project,"code":code}),
        );
    }
    fn find(&self, path: &str, project: &str) -> Option<history::File> {
        self.files
            .iter()
            .find(|f| f.path == path && f.project.as_deref() == Some(project))
            .cloned()
    }
    fn file_node(&mut self, f: &history::File) -> String {
        self.node("source_file", &f.path, &f.id, evidence(f, 0, 0, 1))
    }
    fn local_file(
        &mut self,
        source: &history::File,
        base: &Path,
        relative: &str,
    ) -> Option<history::File> {
        if !literal(relative) || Path::new(relative).is_absolute() {
            self.diagnostic(source, "dynamic_or_nonlocal_path");
            return None;
        }
        let path = normalized(&base.join(relative))?;
        let found = self.find(&path, source.project.as_deref()?);
        if found.is_none() {
            self.diagnostic(source, "referenced_file_not_captured_or_stale");
        }
        found
    }
    fn link_code(&mut self, file: &history::File, from: &str, e: Evidence) {
        let id = self.file_node(file);
        self.edge(from, &id, "references_source_file", e);
        let symbols: Vec<_> = self
            .code
            .get(&file.id)
            .into_iter()
            .flat_map(|d| &d.symbols)
            .filter(|s| s.name == "main" || s.name == "Main")
            .cloned()
            .collect();
        for symbol in symbols {
            let node = self.node(
                "code_symbol",
                &symbol.qualified_name,
                &symbol.id,
                evidence(file, symbol.start_byte, symbol.end_byte, symbol.start_line),
            );
            self.nodes.get_mut(&node).unwrap().symbol_id = Some(symbol.id.clone());
            self.edge(
                &id,
                &node,
                "defines_entrypoint_candidate",
                evidence(file, symbol.start_byte, symbol.end_byte, symbol.start_line),
            );
        }
    }
    fn compose(&mut self, file: &history::File, root: &Yaml) {
        let Some(services) = root.get("services").and_then(Yaml::map) else {
            return;
        };
        for (service, config) in services {
            if !name(service) || config.map().is_none() {
                self.diagnostic(file, "unsupported_compose_service");
                continue;
            }
            let e = anchor(file, config);
            let scope = format!("{}:{}", file.id, root.start);
            let s = self.node("compose_service", service, &scope, e.clone());
            let c = self.node("container", service, &format!("{scope}:compose"), e.clone());
            self.edge(&s, &c, "defines_container", e.clone());
            let origin = self.file_node(file);
            self.edge(&origin, &s, "declares_resource", e.clone());
            let image_id = config
                .get("image")
                .and_then(|v| {
                    v.scalar()
                        .filter(|s| image(s))
                        .map(|value| (value.to_string(), anchor(file, v)))
                })
                .map(|(value, e)| {
                    let id = self.node("image", &value, "", e.clone());
                    self.edge(&c, &id, "uses_image", e);
                    id
                });
            if config.get("image").is_some() && image_id.is_none() {
                self.diagnostic(file, "dynamic_or_invalid_compose_image");
            }
            if let Some(build) = config.get("build") {
                if build.get("dockerfile_inline").is_some() {
                    self.diagnostic(file, "inline_dockerfile_unmodeled");
                    continue;
                }
                let context = build
                    .scalar()
                    .or_else(|| build.get("context").and_then(Yaml::scalar));
                if let Some(context) = context.filter(|s| literal(s) && !s.contains("://")) {
                    let base = Path::new(&file.path).parent().unwrap_or(Path::new(""));
                    let Some(context) = normalized(&base.join(context))
                        .filter(|_| !Path::new(context).is_absolute())
                    else {
                        self.diagnostic(file, "unsupported_build_context");
                        continue;
                    };
                    if build
                        .get("dockerfile")
                        .is_some_and(|v| v.scalar().is_none())
                    {
                        self.diagnostic(file, "invalid_build_dockerfile");
                        continue;
                    }
                    let docker = build
                        .get("dockerfile")
                        .and_then(Yaml::scalar)
                        .unwrap_or("Dockerfile");
                    if let Some(target) = self.local_file(file, Path::new(&context), docker) {
                        let d = self.node(
                            "dockerfile",
                            &target.path,
                            &target.id,
                            evidence(&target, 0, 0, 1),
                        );
                        self.edge(&c, &d, "builds_from", anchor(file, build));
                        if let Some(i) = &image_id {
                            self.edge(i, &d, "declared_build_recipe", anchor(file, build));
                        }
                        if build
                            .get("target")
                            .is_some_and(|v| v.scalar().is_none_or(|s| !name(s)))
                        {
                            self.diagnostic(file, "invalid_build_target");
                            continue;
                        }
                        let target_name = build.get("target").and_then(Yaml::scalar);
                        self.docker(&target, &context, &d, target_name);
                    }
                } else {
                    self.diagnostic(file, "dynamic_or_missing_build_context");
                }
            }
        }
    }
    fn kubernetes(
        &mut self,
        file: &history::File,
        root: &Yaml,
        services: &mut Vec<SelectorRecord>,
        workloads: &mut Vec<SelectorRecord>,
    ) {
        let Some(kind) = root.get("kind").and_then(Yaml::scalar) else {
            return;
        };
        let Some(resource) = root
            .at(&["metadata", "name"])
            .and_then(Yaml::scalar)
            .filter(|n| name(n))
        else {
            self.diagnostic(file, "kubernetes_name_unavailable");
            return;
        };
        if root
            .at(&["metadata", "namespace"])
            .is_some_and(|v| v.scalar().is_none())
        {
            self.diagnostic(file, "invalid_namespace");
            return;
        }
        let namespace = root
            .at(&["metadata", "namespace"])
            .and_then(Yaml::scalar)
            .unwrap_or("default");
        if !name(namespace) {
            self.diagnostic(file, "dynamic_namespace");
            return;
        }
        let group = format!("{}:{namespace}", file.project.as_deref().unwrap_or(""));
        let scope = format!("{}:{}:{kind}:{namespace}:{resource}", file.id, root.start);
        if kind == "Service" {
            let s = self.node("kubernetes_service", resource, &scope, anchor(file, root));
            let origin = self.file_node(file);
            self.edge(&origin, &s, "declares_resource", anchor(file, root));
            if let Some(select) = root
                .at(&["spec", "selector"])
                .and_then(labels)
                .filter(|m| !m.is_empty())
            {
                services.push((group, s, select, anchor(file, root)));
            } else {
                self.diagnostic(file, "kubernetes_selector_unavailable");
            }
            return;
        }
        let (template, podspec) = match kind {
            "Deployment" | "StatefulSet" | "DaemonSet" | "Job" => (
                root.at(&["spec", "template"]),
                root.at(&["spec", "template", "spec"]),
            ),
            "Pod" => (Some(root), root.get("spec")),
            "CronJob" => (
                root.at(&["spec", "jobTemplate", "spec", "template"]),
                root.at(&["spec", "jobTemplate", "spec", "template", "spec"]),
            ),
            _ => return,
        };
        let (Some(template), Some(podspec)) = (template, podspec) else {
            self.diagnostic(file, "pod_template_unavailable");
            return;
        };
        let workload = self.node("kubernetes_workload", resource, &scope, anchor(file, root));
        let origin = self.file_node(file);
        self.edge(&origin, &workload, "declares_resource", anchor(file, root));
        if let Some(labels) = template.at(&["metadata", "labels"]).and_then(labels) {
            workloads.push((group, workload.clone(), labels, anchor(file, template)));
        }
        for key in ["containers", "initContainers"] {
            for container in podspec.get(key).and_then(Yaml::seq).unwrap_or(&[]) {
                let Some(cname) = container
                    .get("name")
                    .and_then(Yaml::scalar)
                    .filter(|s| name(s))
                else {
                    self.diagnostic(file, "container_name_unavailable");
                    continue;
                };
                let c = self.node(
                    "container",
                    cname,
                    &format!("{scope}:{key}:{}", container.start),
                    anchor(file, container),
                );
                self.edge(&workload, &c, "defines_container", anchor(file, container));
                if let Some(v) = container.get("image") {
                    if let Some(value) = v.scalar().filter(|s| image(s)) {
                        let image = self.node("image", value, "", anchor(file, v));
                        self.edge(&c, &image, "uses_image", anchor(file, v));
                    } else {
                        self.diagnostic(file, "dynamic_or_invalid_container_image");
                    }
                }
            }
        }
    }
}
fn labels(n: &Yaml) -> Option<BTreeMap<String, String>> {
    n.map()?
        .iter()
        .map(|(k, v)| Some((k.clone(), v.scalar()?.to_string())))
        .collect()
}

impl Store {
    pub fn code_infrastructure(&self, q: &InfrastructureQuery) -> Result<Value> {
        if q.project.trim().is_empty() {
            return Err(AppError::Invalid("project is required".into()));
        }
        let mut projects = BTreeSet::from([q.project.clone()]);
        projects.extend(q.linked_projects.clone().unwrap_or_default());
        if projects.len() > 9 || projects.iter().any(|p| p.trim().is_empty()) {
            return Err(AppError::Invalid(
                "at most eight explicit linked projects".into(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let latest: Option<i64> =
            tx.query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?;
        let revision = q
            .index_revision
            .or(latest)
            .ok_or_else(|| AppError::Invalid("run index_code first".into()))?;
        let mut files = Vec::new();
        for p in &projects {
            files.extend(history::files(&tx, revision, Some(p))?);
        }
        if files.len() > 1000
            || files.iter().map(|f| f.source.len()).sum::<usize>() > 32 * 1024 * 1024
        {
            return Err(AppError::Invalid(
                "infrastructure capture exceeds 1000 files/32 MiB; no partial graph".into(),
            ));
        }
        let mut diagnostics = Vec::new();
        if q.index_revision.is_none() {
            let captured_ids: HashSet<_> = files.iter().map(|f| f.id.clone()).collect();
            for project in &projects {
                let mut statement=tx.prepare("SELECT id,source_file FROM documents WHERE wing=? AND source_file IS NOT NULL AND COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated') ORDER BY id")?;
                let rows = statement.query_map([project], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (id, path) = row?;
                    if !captured_ids.contains(&id) {
                        diagnostics.push(json!({"document_id":id,"path":path,"project":project,"code":"source_not_published_in_selected_revision"}));
                    }
                }
            }
            files.retain(|f|{
            let current=tx.query_row("SELECT COALESCE(source_file,uri),COALESCE(revision,1),wing,content,metadata_json FROM documents WHERE id=? AND COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated')",[&f.id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional();
            let fresh=current.ok().flatten().is_some_and(|(path,rev,project,content,meta)|path==f.path&&rev==f.revision&&project==f.project&&source::read(&tx,&content,&meta).is_ok_and(|s|content_hash(&s)==f.hash));
            if !fresh{diagnostics.push(json!({"document_id":f.id,"path":f.path,"project":f.project,"code":"stale_or_unavailable_captured_input"}));}fresh
        });
        }
        let mut code = HashMap::new();
        for file in &files {
            if let Some(payload) = &file.analysis {
                if let Ok(doc) = serde_json::from_str::<IndexedDocument>(payload) {
                    if !doc.parse_errors {
                        code.insert(file.id.clone(), doc);
                    }
                }
            }
        }
        let mut graph = Graph {
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            diagnostics,
            files,
            code,
        };
        let mut services = Vec::new();
        let mut workloads = Vec::new();
        for file in graph.files.clone() {
            if !matches!(
                Path::new(&file.path).extension().and_then(|s| s.to_str()),
                Some("yaml" | "yml" | "json")
            ) {
                continue;
            }
            match yaml::documents(&file.source) {
                Ok(docs) => {
                    for doc in docs {
                        graph.compose(&file, &doc);
                        graph.kubernetes(&file, &doc, &mut services, &mut workloads);
                        graph.workflow(&file, &doc);
                    }
                }
                Err(_) => graph.diagnostic(&file, "invalid_or_unsupported_yaml_no_facts"),
            }
        }
        for file in graph.files.clone() {
            let filename = Path::new(&file.path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if (filename == "Dockerfile"
                || filename == "Containerfile"
                || filename.starts_with("Dockerfile.")
                || filename.starts_with("Containerfile.")
                || filename.ends_with(".dockerfile"))
                && !graph
                    .nodes
                    .values()
                    .any(|n| n.kind == "dockerfile" && n.evidence.document_id == file.id)
            {
                let d = graph.node("dockerfile", &file.path, &file.id, evidence(&file, 0, 0, 1));
                graph.diagnostic(&file, "standalone_build_context_unavailable");
                graph.docker(&file, "", &d, None);
            }
        }
        for (group, service, select, e) in services {
            for (wg, workload, labels, _) in &workloads {
                if group == *wg && select.iter().all(|(k, v)| labels.get(k) == Some(v)) {
                    graph.edge(&service, workload, "selector_maps_to", e.clone());
                }
            }
        }
        let images: Vec<_> = graph
            .nodes
            .values()
            .filter(|n| n.kind == "image")
            .cloned()
            .collect();
        for a in &images {
            for b in &images {
                if a.project != b.project && a.name == b.name {
                    graph.edge(
                        &a.id,
                        &b.id,
                        "cross_project_exact_image_candidate",
                        a.evidence.clone(),
                    );
                }
            }
        }
        graph.edges.sort_by(|a, b| {
            (
                &a.source,
                &a.target,
                &a.relation,
                &a.evidence.document_id,
                a.evidence.start_byte,
            )
                .cmp(&(
                    &b.source,
                    &b.target,
                    &b.relation,
                    &b.evidence.document_id,
                    b.evidence.start_byte,
                ))
        });
        graph.edges.dedup_by(|a, b| {
            a.source == b.source
                && a.target == b.target
                && a.relation == b.relation
                && a.evidence.document_id == b.evidence.document_id
                && a.evidence.start_byte == b.evidence.start_byte
        });
        if graph.nodes.len() > 10000 || graph.edges.len() > 20000 {
            return Err(AppError::Invalid(
                "infrastructure graph exceeds node/edge limit; no partial graph".into(),
            ));
        }
        let result = graph.report(q, revision, latest)?;
        tx.commit()?;
        Ok(result)
    }
}

impl Graph {
    fn report(&self, q: &InfrastructureQuery, revision: i64, latest: Option<i64>) -> Result<Value> {
        if q.from.is_some() != q.to.is_some() {
            return Err(AppError::Invalid(
                "from and to must be supplied together".into(),
            ));
        }
        let limit = q.limit.unwrap_or(100).clamp(1, 500);
        let depth = q.max_depth.unwrap_or(10).clamp(1, 30);
        let max_paths = q.max_paths.unwrap_or(3).clamp(1, 20);
        let budget = q.max_work.unwrap_or(20000).clamp(1, 200000);
        let mut out = json!({"contract_version":1,"status":"graph","index_revision":revision,"latest_index_revision":latest,"view":if q.index_revision.is_some(){"frozen_captured_revision"}else{"fresh_subset_of_captured_revision"},"projects":std::iter::once(&q.project).chain(q.linked_projects.iter().flatten()).collect::<Vec<_>>(),"nodes":self.nodes.values().take(limit).collect::<Vec<_>>(),"nodes_total":self.nodes.len(),"nodes_truncated":self.nodes.len()>limit,"edges":self.edges.iter().take(limit).collect::<Vec<_>>(),"edges_total":self.edges.len(),"edges_truncated":self.edges.len()>limit,"diagnostics":self.diagnostics.iter().take(limit).collect::<Vec<_>>(),"diagnostics_total":self.diagnostics.len(),"paths":[],"work":0,"work_exhausted":false,"depth_limited":false,"paths_truncated":false,"coverage_complete":false,"runtime_proven":false,"absence_proven":false,"filesystem_checked":false,"limits":{"depth":depth,"paths":max_paths,"work":budget},"limitations":["captured static candidates only; no cluster, registry, Docker, shell or build execution","templates/variables/aliases/unsupported YAML and Docker forms may leave gaps","image names are declarations, not content identity or observed deployment","new uncaptured files and runtime overrides are not discovered","explicit cross-project linking matches literal image names only"]});
        let (Some(from), Some(to)) = (&q.from, &q.to) else {
            return Ok(out);
        };
        let select = |s: &str| {
            let exact = self.nodes.get(s);
            if let Some(n) = exact {
                vec![n.id.clone()]
            } else {
                self.nodes
                    .values()
                    .filter(|n| n.name == s)
                    .map(|n| n.id.clone())
                    .collect::<Vec<_>>()
            }
        };
        let start = select(from);
        let finish = select(to);
        out["from_candidates"] = json!(start.iter().take(50).collect::<Vec<_>>());
        out["from_candidates_total"] = json!(start.len());
        out["to_candidates"] = json!(finish.iter().take(50).collect::<Vec<_>>());
        out["to_candidates_total"] = json!(finish.len());
        if start.len() != 1 || finish.len() != 1 {
            out["status"] = json!(if start.len() > 1 || finish.len() > 1 {
                "ambiguous_endpoints"
            } else {
                "endpoint_unavailable"
            });
            return Ok(out);
        }
        let mut adjacency: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, e) in self.edges.iter().enumerate() {
            adjacency.entry(&e.source).or_default().push(i);
        }
        let mut queue = VecDeque::from([(vec![start[0].clone()], Vec::<usize>::new())]);
        let mut paths = Vec::new();
        let mut work = 0;
        let mut exhausted = false;
        let mut limited = false;
        while let Some((nodes, edges)) = queue.pop_front() {
            if nodes.last() == Some(&finish[0]) {
                paths.push(json!({"nodes":nodes.iter().map(|id|&self.nodes[id]).collect::<Vec<_>>(),"edges":edges.iter().map(|i|&self.edges[*i]).collect::<Vec<_>>()}));
                if paths.len() == max_paths {
                    limited = !queue.is_empty();
                    break;
                }
                continue;
            }
            if edges.len() >= depth {
                out["depth_limited"] = json!(true);
                continue;
            }
            for index in adjacency
                .get(nodes.last().unwrap().as_str())
                .into_iter()
                .flatten()
            {
                if work >= budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                let e = &self.edges[*index];
                if nodes.contains(&e.target) {
                    continue;
                }
                let mut next_nodes = nodes.clone();
                next_nodes.push(e.target.clone());
                let mut next_edges = edges.clone();
                next_edges.push(*index);
                queue.push_back((next_nodes, next_edges));
            }
            if exhausted {
                break;
            }
        }
        out["status"] = json!(if paths.is_empty() {
            "no_path_in_examined_graph"
        } else {
            "paths"
        });
        out["paths"] = json!(paths);
        out["work"] = json!(work);
        out["work_exhausted"] = json!(exhausted);
        out["paths_truncated"] = json!(limited);
        Ok(out)
    }
}
