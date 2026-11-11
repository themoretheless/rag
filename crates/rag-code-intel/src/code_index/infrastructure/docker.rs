//! Literal Docker/CI commands and captured container-path mappings. Never executes.
use super::*;
#[derive(Clone)]
struct Stage {
    name: String,
    workdir: Option<String>,
    copies: Vec<(String, String, Vec<Evidence>)>,
    entry: Option<Vec<String>>,
    cmd: Option<Vec<String>>,
    e: Evidence,
    shell_known: bool,
    command_e: Option<Evidence>,
    entry_e: Option<Evidence>,
    build_env_known: bool,
    entry_exec: bool,
    cmd_exec: bool,
}
fn container(base: &str, path: &str) -> Option<String> {
    if !literal(path) {
        return None;
    }
    normalized(&Path::new(base).join(path)).filter(|p| p.starts_with('/'))
}
fn words(raw: &str) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in raw.chars() {
        if matches!(
            c,
            '$' | '`' | '\\' | '|' | '&' | ';' | '<' | '>' | '[' | ']' | '*' | '?'
        ) || c.is_control()
        {
            return None;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            started = true;
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                result.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        result.push(word);
    }
    Some(result)
}
fn exec_words(raw: &str) -> Option<Vec<String>> {
    if raw.trim_start().starts_with('[') {
        let v: Vec<String> = serde_json::from_str(raw).ok()?;
        return v.iter().all(|s| literal(s)).then_some(v);
    }
    words(raw)
}
struct DockerBuild<'a> {
    context: &'a str,
    file: Option<&'a str>,
    tag: Option<&'a str>,
    target: Option<&'a str>,
}
fn build_args(args: &[String]) -> Option<DockerBuild<'_>> {
    let mut out = DockerBuild {
        context: ".",
        file: None,
        tag: None,
        target: None,
    };
    let mut at = 2;
    let mut context = false;
    while at < args.len() {
        let slot = match args[at].as_str() {
            "-f" | "--file" => Some(&mut out.file),
            "-t" | "--tag" => Some(&mut out.tag),
            "--target" => Some(&mut out.target),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_some() {
                return None;
            }
            let value = args.get(at + 1)?.as_str();
            if !literal(value) {
                return None;
            }
            *slot = Some(value);
            at += 2;
        } else if !args[at].starts_with('-') && !context && at == args.len() - 1 {
            out.context = &args[at];
            context = true;
            at += 1;
        } else {
            return None;
        }
    }
    context.then_some(out)
}
fn mapped(
    graph: &Graph,
    stage: &Stage,
    path: &str,
    project: &str,
) -> Option<(history::File, Vec<Evidence>)> {
    for (dest, source, evidence) in stage.copies.iter().rev() {
        if path == dest {
            return graph.find(source, project).map(|f| (f, evidence.clone()));
        }
        if let Ok(suffix) = Path::new(path).strip_prefix(dest) {
            if graph.find(source, project).is_some() {
                return None;
            }
            return normalized(&Path::new(source).join(suffix))
                .and_then(|path| graph.find(&path, project))
                .map(|f| (f, evidence.clone()));
        }
    }
    None
}
fn cargo_target(
    graph: &Graph,
    file: &history::File,
    manifest: &history::File,
    args: &[String],
) -> Option<(String, history::File)> {
    if !args.first().is_some_and(|s| s == "cargo") || !args.get(1).is_some_and(|s| s == "build") {
        return None;
    }
    let bin = args.windows(2).find(|v| v[0] == "--bin")?.get(1)?;
    if !name(bin) || bin.contains('/') {
        return None;
    }
    let mut at = 2;
    let mut seen = HashSet::new();
    while at < args.len() {
        if !seen.insert(args[at].as_str()) {
            return None;
        }
        match args[at].as_str() {
            "--bin" | "--manifest-path" => {
                args.get(at + 1)?;
                at += 2;
            }
            "--release" | "--locked" | "--offline" | "--frozen" => at += 1,
            _ => return None,
        }
    }
    // Workspace target directories and build-time overrides need a separate model.
    if graph.files.iter().any(|f| {
        f.project == file.project
            && f.path.ends_with("Cargo.toml")
            && Path::new(&manifest.path).starts_with(Path::new(&f.path).parent().unwrap())
            && f.source.contains("[workspace]")
    }) {
        return None;
    }
    let targets = rust_cargo::target_evidence(&graph.code);
    let found: Vec<_> = targets
        .iter()
        .filter(|t| {
            t["project"].as_str() == file.project.as_deref()
                && t["manifest_path"].as_str() == Some(manifest.path.as_str())
                && t["kind"] == "bin"
                && t["name"].as_str() == Some(bin.replace('-', "_").as_str())
                && t["required_features_unselected"] == false
        })
        .collect();
    if found.len() != 1 {
        return None;
    }
    let source = graph.find(found[0]["root_path"].as_str()?, file.project.as_deref()?)?;
    Some((bin.clone(), source))
}
impl Graph {
    pub(super) fn docker(
        &mut self,
        file: &history::File,
        context: &str,
        parent: &str,
        target: Option<&str>,
    ) {
        let project = file.project.as_deref().unwrap_or("");
        let mut stages = Vec::<Stage>::new();
        let mut offset = 0;
        let mut pending = String::new();
        let mut pending_start = 0;
        let mut pending_line = 1;
        for (line_index, line) in file.source.split_inclusive('\n').enumerate() {
            if line_index >= 10000 || stages.len() > 128 {
                self.diagnostic(file, "docker_instruction_or_stage_limit");
                return;
            }
            let start = offset;
            offset += line.len();
            let clean = line.trim();
            if pending.is_empty() && (clean.is_empty() || clean.starts_with('#')) {
                continue;
            }
            if pending.is_empty() {
                pending_start = start;
                pending_line = line_index + 1;
            }
            let continuation = clean.ends_with('\\');
            pending.push_str(if continuation {
                clean.trim_end_matches('\\')
            } else {
                clean
            });
            pending.push(' ');
            if continuation {
                continue;
            }
            let instruction = std::mem::take(&mut pending);
            let mut parts = instruction.trim().splitn(2, char::is_whitespace);
            let verb = parts.next().unwrap_or("").to_ascii_uppercase();
            let raw = parts.next().unwrap_or("").trim();
            let e = evidence(file, pending_start, offset, pending_line);
            if verb == "FROM" {
                if stages.len() >= 128 {
                    self.diagnostic(file, "docker_instruction_or_stage_limit");
                    return;
                }
                let Some(args) = words(raw) else {
                    self.diagnostic(file, "dynamic_docker_from");
                    return;
                };
                if args.len() != 1 && !(args.len() == 3 && args[1].eq_ignore_ascii_case("as")) {
                    self.diagnostic(file, "unsupported_docker_from_flags");
                    return;
                }
                if !image(&args[0]) {
                    self.diagnostic(file, "invalid_docker_base_image");
                    return;
                }
                let label = args
                    .get(2)
                    .cloned()
                    .unwrap_or_else(|| format!("stage{}", stages.len()));
                if !name(&label) || stages.iter().any(|s| s.name == label) {
                    self.diagnostic(file, "ambiguous_docker_stage_name");
                    return;
                }
                let base = stages.iter().find(|s| s.name == args[0]).cloned();
                stages.push(if let Some(mut s) = base {
                    s.name = label;
                    s.e = e;
                    s
                } else {
                    Stage {
                        name: label,
                        workdir: Some("/".into()),
                        copies: Vec::new(),
                        entry: None,
                        cmd: None,
                        e,
                        shell_known: true,
                        command_e: None,
                        entry_e: None,
                        build_env_known: true,
                        entry_exec: false,
                        cmd_exec: false,
                    }
                });
                continue;
            }
            let Some((current, previous)) = stages.split_last_mut() else {
                if verb != "ARG" {
                    self.diagnostic(file, "instruction_before_from");
                }
                continue;
            };
            match verb.as_str() {
                "WORKDIR" => {
                    current.workdir = current
                        .workdir
                        .as_ref()
                        .and_then(|base| container(base, raw));
                    if current.workdir.is_none() {
                        self.diagnostic(file, "dynamic_docker_workdir");
                    }
                }
                "COPY" => {
                    let Some(mut args) = exec_words(raw) else {
                        current.copies.clear();
                        self.diagnostic(file, "dynamic_docker_copy");
                        continue;
                    };
                    let from = if !raw.starts_with('[')
                        && args.first().is_some_and(|s| s.starts_with("--from="))
                    {
                        Some(args.remove(0).trim_start_matches("--from=").to_string())
                    } else {
                        if context.is_empty() {
                            current.copies.clear();
                            self.diagnostic(file, "docker_context_unavailable");
                            continue;
                        }
                        None
                    };
                    if args.len() != 2
                        || args.iter().any(|a| {
                            a.contains(['*', '?', '[']) || a.starts_with("--") || !literal(a)
                        })
                    {
                        current.copies.clear();
                        self.diagnostic(file, "unsupported_docker_copy");
                        continue;
                    }
                    let Some(base) = current.workdir.as_deref() else {
                        current.copies.clear();
                        continue;
                    };
                    let Some(mut dest) = container(base, &args[1]) else {
                        current.copies.clear();
                        continue;
                    };
                    if let Some(from) = from {
                        let prior = previous.iter().find(|s| s.name == from).cloned();
                        let Some(prior) = prior else {
                            current.copies.clear();
                            self.diagnostic(file, "external_or_unknown_copy_stage");
                            continue;
                        };
                        let Some((source, mut support)) = container("/", &args[0])
                            .and_then(|p| mapped(self, &prior, &p, project))
                        else {
                            current.copies.clear();
                            self.diagnostic(file, "copy_stage_source_unavailable");
                            continue;
                        };
                        if args[1].ends_with('/') {
                            dest = container(
                                &dest,
                                Path::new(&args[0])
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(""),
                            )
                            .unwrap_or(dest);
                        }
                        support.push(e.clone());
                        current.copies.push((dest, source.path, support));
                    } else {
                        if Path::new(&args[0]).is_absolute() || args[0].contains("://") {
                            current.copies.clear();
                            self.diagnostic(file, "nonlocal_docker_copy");
                            continue;
                        }
                        let Some(source) = normalized(&Path::new(context).join(&args[0]))
                            .filter(|s| Path::new(s).starts_with(context))
                        else {
                            current.copies.clear();
                            self.diagnostic(file, "docker_copy_context_escape");
                            continue;
                        };
                        if self.find(&source, project).is_some() && args[1].ends_with('/') {
                            dest = container(
                                &dest,
                                Path::new(&source)
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(""),
                            )
                            .unwrap_or(dest);
                        }
                        current.copies.push((dest, source, vec![e.clone()]));
                    }
                }
                "ADD" => {
                    current.copies.clear();
                    self.diagnostic(file, "docker_add_unmodeled");
                }
                "ENTRYPOINT" => {
                    current.entry_e = Some(e.clone());
                    current.entry_exec = raw.starts_with('[');
                    current.entry = exec_words(raw).filter(|v| !v.is_empty());
                    if current.entry.is_none() {
                        self.diagnostic(file, "dynamic_docker_entrypoint");
                    }
                }
                "CMD" => {
                    current.command_e = Some(e.clone());
                    current.cmd_exec = raw.starts_with('[');
                    current.cmd = exec_words(raw).filter(|v| !v.is_empty());
                    if current.cmd.is_none() {
                        self.diagnostic(file, "dynamic_docker_cmd");
                    }
                }
                "SHELL" => {
                    current.shell_known = false;
                    self.diagnostic(file, "custom_docker_shell");
                }
                "RUN" => {
                    let args = if current.build_env_known
                        && (current.shell_known || raw.starts_with('['))
                    {
                        exec_words(raw)
                    } else {
                        None
                    };
                    let result = args.as_ref().and_then(|args| {
                        let workdir = current.workdir.as_deref()?;
                        let manifest = args
                            .windows(2)
                            .find(|v| v[0] == "--manifest-path")
                            .map(|v| v[1].as_str())
                            .unwrap_or("Cargo.toml");
                        let (manifest, mut support) = container(workdir, manifest)
                            .and_then(|p| mapped(self, current, &p, project))?;
                        let (binary, source) = cargo_target(self, file, &manifest, args)?;
                        support.push(evidence(&manifest, 0, manifest.source.len(), 1));
                        support.push(e.clone());
                        let profile = if args.iter().any(|a| a == "--release") {
                            "release"
                        } else {
                            "debug"
                        };
                        Some((
                            container(workdir, &format!("target/{profile}/{binary}"))?,
                            source.path,
                            support,
                        ))
                    });
                    if let Some(mapping) = result {
                        current.copies.push(mapping);
                    } else {
                        current.copies.clear();
                        self.diagnostic(file, "unmodeled_run_invalidates_container_source_mapping");
                    }
                }
                "ARG" | "ENV" => {
                    current.build_env_known = false;
                }
                "LABEL" | "EXPOSE" | "USER" | "HEALTHCHECK" | "STOPSIGNAL" | "VOLUME"
                | "MAINTAINER" => {}
                _ => {
                    current.copies.clear();
                    self.diagnostic(file, "unsupported_docker_instruction");
                }
            }
        }
        if !pending.trim().is_empty() {
            self.diagnostic(file, "incomplete_docker_continuation");
            return;
        }
        let selected = if let Some(target) = target {
            stages.iter().find(|s| s.name == target)
        } else {
            stages.last()
        };
        let Some(stage) = selected else {
            self.diagnostic(file, "docker_target_unavailable");
            return;
        };
        let scope = format!("{}:{context}:{}", file.id, stage.name);
        let id = self.node("docker_stage", &stage.name, &scope, stage.e.clone());
        self.edge(parent, &id, "selects_build_stage", stage.e.clone());
        let mut command = if stage.entry_e.is_some() {
            stage.entry.clone()
        } else {
            stage.cmd.clone()
        };
        if stage.entry.is_some() && stage.entry_exec && stage.cmd_exec {
            if let (Some(args), Some(cmd)) = (&mut command, &stage.cmd) {
                args.extend(cmd.clone());
            }
        }
        let Some(args) = command else {
            self.diagnostic(file, "docker_default_command_unavailable");
            return;
        };
        let command_e = stage
            .entry_e
            .clone()
            .or_else(|| stage.command_e.clone())
            .unwrap_or_else(|| stage.e.clone());
        let executable = self.node("executable", &args[0], &scope, command_e.clone());
        self.edge(
            &id,
            &executable,
            "default_execution_candidate",
            command_e.clone(),
        );
        let program = Path::new(&args[0])
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let target = if matches!(program, "python" | "python3" | "node" | "bash" | "sh") {
            args.get(1).filter(|p| !p.starts_with('-'))
        } else {
            args.first().filter(|p| p.contains('/'))
        };
        if let Some((target, support)) = target
            .and_then(|target| {
                stage
                    .workdir
                    .as_deref()
                    .and_then(|base| container(base, target))
            })
            .and_then(|p| mapped(self, stage, &p, project))
        {
            self.link_code(&target, &executable, command_e.clone());
            if let Some(edge) =
                self.edges.iter_mut().rev().find(|edge| {
                    edge.source == executable && edge.relation == "references_source_file"
                })
            {
                edge.supporting_evidence = support;
            }
        } else {
            self.diagnostic(file, "executable_source_unavailable");
        }
    }
    pub(super) fn workflow(&mut self, file: &history::File, root: &Yaml) {
        let Some((repository, _)) = file.path.split_once("/.github/workflows/") else {
            return;
        };
        let Some(jobs) = root.get("jobs").and_then(Yaml::map) else {
            return;
        };
        for (name, job) in jobs {
            if !super::name(name) {
                self.diagnostic(file, "invalid_ci_job_name");
                continue;
            }
            let id = self.node("ci_job", name, &file.id, anchor(file, job));
            for step in job.get("steps").and_then(Yaml::seq).unwrap_or(&[]) {
                let Some(run) = step.get("run") else {
                    continue;
                };
                let directory = step
                    .get("working-directory")
                    .or_else(|| job.at(&["defaults", "run", "working-directory"]))
                    .or_else(|| root.at(&["defaults", "run", "working-directory"]))
                    .and_then(Yaml::scalar)
                    .unwrap_or(".");
                if !literal(directory) || Path::new(directory).is_absolute() {
                    self.diagnostic(file, "dynamic_ci_working_directory");
                    continue;
                }
                let Some(base) = normalized(&Path::new(repository).join(directory))
                    .filter(|p| Path::new(p).starts_with(repository))
                else {
                    self.diagnostic(file, "ci_working_directory_escape");
                    continue;
                };
                let lines: Vec<_> = match &run.data {
                    Data::Scalar(s) => vec![s.as_str()],
                    Data::Block(s) if s.starts_with('|') => s.lines().skip(1).collect(),
                    _ => {
                        self.diagnostic(file, "unsupported_ci_run");
                        continue;
                    }
                };
                for raw in lines {
                    let Some(args) = words(raw.trim()) else {
                        self.diagnostic(file, "dynamic_or_compound_ci_command");
                        continue;
                    };
                    if args.is_empty() {
                        continue;
                    }
                    if args.first().is_some_and(|a| a == "docker")
                        && args.get(1).is_some_and(|a| a == "build")
                    {
                        let Some(build) = build_args(&args) else {
                            self.diagnostic(file, "unsupported_ci_docker_build_flags");
                            continue;
                        };
                        let context = build.context;
                        if !literal(context) || Path::new(context).is_absolute() {
                            self.diagnostic(file, "unsupported_ci_build_context");
                            continue;
                        }
                        let Some(context) = normalized(&Path::new(&base).join(context))
                            .filter(|p| Path::new(p).starts_with(repository))
                        else {
                            continue;
                        };
                        let docker = build.file;
                        let target = if let Some(docker) = docker {
                            self.local_file(file, Path::new(&base), docker)
                        } else {
                            self.local_file(file, Path::new(&context), "Dockerfile")
                        };
                        if let Some(target) = target {
                            let d = self.node(
                                "dockerfile",
                                &target.path,
                                &target.id,
                                evidence(&target, 0, 0, 1),
                            );
                            self.edge(&id, &d, "builds_from", anchor(file, run));
                            if let Some(tag) = build.tag.filter(|s| image(s)) {
                                let image = self.node("image", tag, "", anchor(file, run));
                                self.edge(&image, &d, "declared_build_recipe", anchor(file, run));
                            }
                            let stage = build.target;
                            self.docker(&target, &context, &d, stage);
                        }
                    } else {
                        let path = if matches!(
                            args[0].as_str(),
                            "bash" | "sh" | "python" | "python3" | "node"
                        ) {
                            args.get(1).filter(|s| !s.starts_with('-'))
                        } else {
                            args.first().filter(|s| s.starts_with("./"))
                        };
                        if let Some(target) =
                            path.and_then(|path| self.local_file(file, Path::new(&base), path))
                        {
                            self.link_code(&target, &id, anchor(file, run));
                        }
                    }
                }
            }
        }
    }
}
