# Captured infrastructure graph, contract 1

`infrastructure` is a read-authorized full-surface MCP tool. It reads immutable
captured source files and cached code facts under one database transaction. It
does not walk/read the live source tree, run Docker/Cargo/shell commands, contact
a cluster or registry, or infer observed execution. An ordinary query requires
`project`; `linked_projects` explicitly admits at most eight additional projects.
Default queries exclude captured files whose current stored identity, project,
revision or decoded source hash no longer matches. New stored source files not
in the published revision are diagnosed. An explicit `index_revision` instead
reads the frozen captured view, including old input bodies and cached code facts.
Configuration edges are derived by the current contract implementation from
those frozen inputs; a separately persisted infrastructure-edge snapshot/diff
format is not implemented.

## Nodes and relationships

- Compose service → declared container → literal image; local build context and
  Dockerfile declarations add `builds_from` and `declared_build_recipe` candidates.
- Kubernetes Service → workload by a nonempty exact selector contained in pod
  labels, within the same project and namespace. Deployment, StatefulSet,
  DaemonSet, Job, CronJob and Pod templates contribute regular/init containers and
  literal image references. Missing namespace defaults only when absent, not
  when a present value is invalid. Duplicate declarations retain separate source
  identities and produce endpoint ambiguity rather than merging resources.
- Dockerfile → selected stage → literal default executable → captured source
  file → extracted `main`/`Main` entrypoint candidates. A `code_symbol` node
  includes its original `symbol_id` for subsequent ordinary code-graph queries.
- GitHub Actions job → literal Docker build recipe or directly named local script.
  Supported working-directory declarations resolve from the workflow's repository
  root and cannot escape it. Literal `docker build` flags are parsed explicitly;
  unknown/duplicate flags and dynamic/compound shell commands are not guessed.
- Explicit linked projects add directed `cross_project_exact_image_candidate`
  edges for equal literal image names. Names/tags do not prove image-content
  identity, deployment freshness or a network connection.

Images are project-scoped literal concepts with incoming source evidence;
container/workload/service declarations retain source identity. Every edge is
`captured_static_candidate` with `runtime_proven=false`. Evidence contains document,
project, path, captured revision/hash, decoded UTF-8 byte offsets and line. A
compiled executable binding retains supporting COPY, Cargo manifest, RUN and
inter-stage COPY evidence; the execution edge points at ENTRYPOINT/CMD itself.
Responses do not return command arguments, environment values, source bodies or
raw parse errors. Literal noncredential image names and supported resource names
are returned; templated/dynamic image values are rejected without echoing them.

## Parsing and build limits

YAML uses tree-sitter's YAML grammar, retaining original anchors. Block/flow maps,
sequences, ordinary quoted/plain strings and multiple documents are supported.
Syntax errors, duplicate keys, anchors/aliases, merge keys, tags, complex keys and
unsupported scalar escapes reject facts for that file. Literal workflow `|`
blocks can provide separate command lines; folded shell/control-flow evaluation
is not implemented. JSON manifests use the same structural parser.

Docker parsing handles literal FROM/stage aliases, WORKDIR, two-operand local
COPY (including JSON arrays), known inter-stage single-file COPY, ENTRYPOINT/CMD
and a constrained explicit `cargo build --bin` target. Captured Cargo metadata
must identify exactly one bin/root in the mapped manifest. Release/debug output
mapping is bounded; workspace output directories, custom profiles/targets,
unknown flags and build environment overrides remain unsupported. Unmodeled RUN,
ADD or COPY operations invalidate affected container-source mappings rather than
retaining an earlier potentially overwritten source. Known mappings use the
latest covering COPY and never fall back through an unresolved shadowing copy.

Standalone Dockerfiles expose declared stage/executable facts but do not assume
a build context or bind local source until a captured Compose/CI recipe supplies
one. Dockerfile, Dockerfile.*, Containerfile, Containerfile.* and *.dockerfile
files are eligible for source ingestion. Runtime ENTRYPOINT overrides, external
base-image content, .dockerignore selection, symlinks inside images, remote
contexts, shell expansion and actual image contents remain unverified. Paths are
static source candidates, not guarantees that a file reached or ran in an image.

## Queries and bounds

Omit `from`/`to` for a bounded inventory; supply both for directed simple paths.
Selectors accept exact infrastructure node IDs or names. Multiple matches return
`ambiguous_endpoints`; unavailable ones are explicit. Paths retain full node/edge
evidence and code-symbol identity. No result proves absence or complete coverage.

Limits: 1000 captured files / 32 MiB total source; YAML depth 64 and 100000 value
steps per file; Docker 10000 lines / 128 stages; 10000 graph nodes / 20000 edges.
Construction overflow refuses an applicable partial graph. Inventory output is
1–500 nodes/edges/diagnostics per list with total/truncation fields. Path depth
is 1–30 (default 10), paths 1–20 (default 3), traversal work 1–200000 (default
20000). Exhausted work/depth/result caps remain explicit. These are logical
bounds, not a qualified physical memory or latency guarantee.

Helm rendering, Kustomize transforms, Terraform/Ansible resources, general Make/
shell execution, runtime traces, image digests fetched from registries and live
deployment state are not implemented by this initial provider set. Infrastructure
edges are queried separately from the existing symbol-only `trace_path` API.
Real-repository extraction quality, broader providers and comparative performance
remain open; the original complete goal is not closed by fixture qualification.

Evidence: [infrastructure qualification](qualification/infrastructure-2026-10-26/README.md).
