use super::*;
struct Fixture {
    store: Store,
    _dir: tempfile::TempDir,
    sources: BTreeMap<String, String>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("infra.duckdb")).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/code-intelligence/infrastructure");
        let mut sources = BTreeMap::new();
        for path in [
            "Cargo.toml",
            "src/main.rs",
            "Dockerfile",
            "compose.yaml",
            "service.yaml",
            "deployment.yaml",
            "wrong-namespace.yaml",
            ".github/workflows/build.yaml",
            "scripts/check.sh",
        ] {
            sources.insert(
                path.into(),
                std::fs::read_to_string(root.join(path)).unwrap(),
            );
        }
        let fixture = Self {
            store,
            _dir: dir,
            sources,
        };
        for (path, source) in &fixture.sources {
            fixture.put("p", path, source, 1);
        }
        fixture.store.index_code().unwrap();
        fixture
    }
    fn put(&self, project: &str, path: &str, source: &str, revision: i64) {
        self.store
            .upsert_document(&crate::models::Document {
                id: format!("{project}:{path}"),
                source_file: Some(format!("/{project}/{path}")),
                uri: format!("file:///{project}/{path}"),
                content: source.into(),
                revision,
                wing: Some(project.into()),
                ..Default::default()
            })
            .unwrap();
    }
    fn query(&self, extra: Value) -> Value {
        let mut q = json!({"project":"p","limit":500});
        for (k, v) in extra.as_object().unwrap() {
            q[k] = v.clone();
        }
        self.store
            .code_infrastructure(&serde_json::from_value::<InfrastructureQuery>(q).unwrap())
            .unwrap()
    }
    fn node(&self, kind: &str, name: &str) -> String {
        let g = self.query(json!({}));
        let found: Vec<_> = g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == kind && n["name"] == name && n["project"] == "p")
            .collect();
        assert_eq!(found.len(), 1, "{g}");
        found[0]["id"].as_str().unwrap().into()
    }
    fn path(&self, extra: Value) -> Value {
        let from = self.node("kubernetes_service", "public-api");
        let to = self.node("code_symbol", "main");
        let mut q = json!({"from":from,"to":to,"max_paths":20});
        for (k, v) in extra.as_object().unwrap() {
            q[k] = v.clone();
        }
        self.query(q)
    }
}
#[test]
fn yaml_ast_handles_blocks_flows_and_rejects_duplicate_alias_and_invalid_forms() {
    let docs = yaml::documents(
        "services: {api: {image: 'demo/api:v1'}}\n---\nkind: Service\nmetadata:\n  name: test\n",
    )
    .unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(
        docs[0]
            .at(&["services", "api", "image"])
            .and_then(Yaml::scalar),
        Some("demo/api:v1")
    );
    for source in [
        "services: {api: {image: first, image: second}}",
        "x: &base {image: demo}\ny: *base",
        "kind: [unclosed",
        "services: !!map {}",
    ] {
        assert!(yaml::documents(source).is_err(), "{source}");
    }
}
#[test]
fn service_image_multistage_executable_code_paths_have_exact_source_support() {
    let f = Fixture::new();
    let result = f.path(json!({}));
    assert_eq!(result["status"], "paths", "{result}");
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/code-intelligence/infrastructure/expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let paths = result["paths"].as_array().unwrap();
    assert!(
        paths.iter().any(|p| json!(p["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["kind"].as_str().unwrap())
            .collect::<Vec<_>>())
            == expected["service_to_main_kinds"]),
        "{result}"
    );
    let path = &paths[0];
    let execution = path["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["relation"] == "default_execution_candidate")
        .unwrap();
    assert_eq!(execution["evidence"]["line"], 8);
    let binding = path["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["relation"] == "references_source_file")
        .unwrap();
    let lines: BTreeSet<_> = binding["supporting_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["path"] == "/p/Dockerfile")
        .map(|e| e["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, BTreeSet::from([3, 4, 7]));
    let graph = f.query(json!({}));
    let wrong = f.node("kubernetes_workload", "wrong-namespace");
    assert!(!graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["relation"] == "selector_maps_to" && e["target"] == wrong));
    let job = f.node("ci_job", "image-build");
    let script = f.node("source_file", "/p/scripts/check.sh");
    assert_eq!(f.query(json!({"from":job,"to":script}))["status"], "paths");
    assert_eq!(result, f.path(json!({})));
    assert_eq!(result["runtime_proven"], false);
}
#[test]
fn stale_bridge_and_manifest_are_excluded_but_frozen_revision_is_retained() {
    let f = Fixture::new();
    let before = f.path(json!({}));
    let revision = before["index_revision"].clone();
    f.put(
        "p",
        "deployment.yaml",
        &f.sources["deployment.yaml"].replace("demo/api:v1", "demo/other:v2"),
        2,
    );
    let stale = f.path(json!({}));
    assert_eq!(stale["status"], "no_path_in_examined_graph", "{stale}");
    assert!(stale["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "stale_or_unavailable_captured_input"));
    assert_eq!(
        f.path(json!({"index_revision":revision}))["status"],
        "paths"
    );
    f.store.index_code().unwrap();
    f.put(
        "p",
        "Cargo.toml",
        &(f.sources["Cargo.toml"].clone() + "\n# changed provider\n"),
        2,
    );
    let current = f.query(json!({}));
    assert!(!current["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["kind"] == "code_symbol"));
}
#[test]
fn cross_project_images_require_explicit_scope_and_path_limits_remain_visible() {
    let f = Fixture::new();
    for (path, source) in &f.sources {
        f.put("other", path, source, 1);
    }
    f.store.index_code().unwrap();
    let local = f.query(json!({}));
    assert!(!local["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["relation"] == "cross_project_exact_image_candidate"));
    let linked = f.query(json!({"linked_projects":["other"]}));
    assert!(linked["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["relation"] == "cross_project_exact_image_candidate"));
    assert_eq!(
        f.query(json!({"from":"main","to":"main","linked_projects":["other"]}))["status"],
        "ambiguous_endpoints"
    );
    let limited = f.path(json!({"max_depth":1}));
    assert_eq!(limited["depth_limited"], true);
    assert_eq!(limited["absence_proven"], false);
    let limited = f.path(json!({"max_work":1}));
    assert_eq!(limited["work_exhausted"], true);
    assert_eq!(limited["work"], 1);
}
#[test]
fn dynamic_invalid_and_unmodeled_container_forms_never_keep_old_code_binding() {
    for docker in [
        "FROM scratch\nCOPY . /app\nRUN rm -rf /app\nENTRYPOINT [\"/app/src/main.rs\"]\n",
        "FROM scratch\nCOPY ../outside /app/api\nENTRYPOINT [\"/app/api\"]\n",
        "FROM scratch\nCOPY . /app\nENTRYPOINT [\"$EXECUTABLE\"]\n",
        "FROM scratch\nCOPY . /app\nCOPY --from=missing /bad /app/src/main.rs\nENTRYPOINT [\"/app/src/main.rs\"]\n",
        "FROM scratch\nCOPY . /app\nCOPY ../outside /app/src/main.rs\nENTRYPOINT [\"/app/src/main.rs\"]\n",
        "FROM scratch\nCOPY . /app\nENTRYPOINT $UNKNOWN\nCMD [\"/app/src/main.rs\"]\n",
        "FROM scratch AS builder\nCOPY . /app\nFROM scratch\nCOPY [\"--from=builder\",\"/app/src/main.rs\",\"/app/main.rs\"]\nENTRYPOINT [\"/app/main.rs\"]\n",
    ] {
        let f = Fixture::new();
        f.put("p", "Dockerfile", docker, 2);
        f.store.index_code().unwrap();
        let g = f.query(json!({}));
        assert!(
            !g["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["kind"] == "code_symbol"),
            "{g}"
        );
    }
    let f = Fixture::new();
    f.put(
        "p",
        "compose.yaml",
        "services: {api: {image: '${SECRET}', build: {context: ., target: [runtime]}}}",
        2,
    );
    f.store.index_code().unwrap();
    let g = f.query(json!({}));
    assert!(!serde_json::to_string(&g).unwrap().contains("SECRET"));
    assert!(!g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["kind"] == "compose_service" && n["name"] == "${SECRET}"));
}

#[test]
fn duplicate_resources_unpublished_sources_and_standalone_context_are_explicit() {
    let f = Fixture::new();
    let duplicate = format!(
        "{}\n---\n{}",
        f.sources["service.yaml"], f.sources["service.yaml"]
    );
    f.put("p", "service.yaml", &duplicate, 2);
    f.store.index_code().unwrap();
    assert_eq!(
        f.query(json!({"from":"public-api","to":"main"}))["status"],
        "ambiguous_endpoints"
    );
    f.put(
        "p",
        "new.yaml",
        "kind: Service\nmetadata: {name: uncaptured}\n",
        1,
    );
    let g = f.query(json!({}));
    assert!(g["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "source_not_published_in_selected_revision"));
    f.put(
        "p",
        "standalone.dockerfile",
        "FROM scratch\nCOPY . /app\nENTRYPOINT [\"/app/src/main.rs\"]\n",
        1,
    );
    f.store.index_code().unwrap();
    let g = f.query(json!({}));
    let stage = g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["kind"] == "docker_stage" && n["evidence"]["path"] == "/p/standalone.dockerfile"
        })
        .unwrap();
    let execution = g["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["source"] == stage["id"] && e["relation"] == "default_execution_candidate")
        .unwrap();
    assert!(!g["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["source"] == execution["target"] && e["relation"] == "references_source_file"));
}

#[test]
fn docker_stage_limit_rejects_overflow_without_publishing_a_partial_stage() {
    let f = Fixture::new();
    for count in [128, 129] {
        let source = (0..count)
            .map(|i| format!("FROM scratch AS stage{i}\n"))
            .collect::<String>();
        f.put("p", "budget.dockerfile", &source, count);
        f.store.index_code().unwrap();
        let graph = f.query(json!({}));
        let overflow = graph["diagnostics"].as_array().unwrap().iter().any(|d| {
            d["path"] == "/p/budget.dockerfile" && d["code"] == "docker_instruction_or_stage_limit"
        });
        assert_eq!(overflow, count == 129);
        let published = graph["nodes"].as_array().unwrap().iter().any(|n| {
            n["kind"] == "docker_stage" && n["evidence"]["path"] == "/p/budget.dockerfile"
        });
        assert_eq!(published, count == 128);
    }
}
