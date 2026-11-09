//! Graph pipeline benchmark on a synthetic 5 000-node / 20 000-edge graph.
//!
//! Not a correctness test: it is `#[ignore]`d and only prints timings.
//!
//! ```text
//! cargo test -p rag-mcp-ui --no-default-features --release --locked -- \
//!     --ignored --nocapture graph_pipeline_benchmark
//! ```
//!
//! The frame numbers come from a headless `egui::Context`: everything the
//! canvas does on the CPU each frame (culling, hit-test, legend, tooltip data
//! and shape generation) without tessellation or a GPU. Results are recorded in
//! `docs/refactor/BASELINE_EGUI.md`.

use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Vec2};
use rag_mcp_core::{GraphEdge, GraphNode, GraphView};

use crate::adapter::{adapt, AdaptOptions, GraphLens, UiGraph};
use crate::layout::{overview_grid_place, radial_place, PosCache};
use crate::ui::canvas::draw_canvas;
use crate::ui::detail::{draw_detail, DetailContentState};
use crate::ui::scene::GraphScene;

const NODES: usize = 5_000;
const EDGES: usize = 20_000;

/// Deterministic pseudo-random stream (no `rand` dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound
    }
}

/// `nodes` nodes (documents across layers and projects, plus tags, stubs and
/// entities) and `edges` edges over a realistic mix of relation types, with a
/// handful of hubs so degree is not uniform.
pub(crate) fn synthetic_view(nodes: usize, edges: usize) -> GraphView {
    const RELS: [&str; 10] = [
        "wikilink",
        "wikilink",
        "wikilink",
        "related",
        "related",
        "tagged",
        "mentions",
        "depends_on",
        "contains",
        "реализует",
    ];
    let mut rng = Lcg(0x5EED);
    let graph_nodes = (0..nodes)
        .map(|index| {
            let (kind, metadata) = match index % 10 {
                0 => ("tag", "{}".to_string()),
                1 => ("stub", "{}".to_string()),
                2 => ("entity", format!(r#"{{"wing":"p{}"}}"#, index % 7)),
                3 | 4 => (
                    "document",
                    format!(
                        r#"{{"layer":"wiki","wing":"p{}","room":"r{}"}}"#,
                        index % 7,
                        index % 23
                    ),
                ),
                _ => (
                    "document",
                    format!(
                        r#"{{"layer":"raw","wing":"p{}","room":"r{}"}}"#,
                        index % 7,
                        index % 23
                    ),
                ),
            };
            GraphNode {
                id: format!("n{index:05}"),
                kind: kind.into(),
                label: format!("Node {index} of the synthetic corpus"),
                document_id: (kind == "document").then(|| format!("d{index:05}")),
                uri: (kind == "document").then(|| format!("file:///corpus/{index}.md")),
                resolved: kind != "stub",
                metadata_json: metadata,
            }
        })
        .collect();
    let graph_edges = (0..edges)
        .map(|index| {
            // Every eighth edge attaches to one of 16 hubs.
            let source = if index % 8 == 0 {
                rng.next(16)
            } else {
                rng.next(nodes)
            };
            let mut target = rng.next(nodes);
            if target == source {
                target = (target + 1) % nodes;
            }
            GraphEdge {
                id: format!("e{index:05}"),
                source_id: format!("n{source:05}"),
                target_id: format!("n{target:05}"),
                rel_type: RELS[rng.next(RELS.len())].into(),
                weight: 1.0 + (index % 5) as f64,
                context: None,
            }
        })
        .collect();
    GraphView {
        nodes: graph_nodes,
        edges: graph_edges,
    }
}

fn overview_options() -> AdaptOptions {
    AdaptOptions {
        lens: GraphLens::Overview,
        show_tags: true,
        show_stubs: true,
        pkb_rels_only: false,
        show_all_nodes: true,
        ..AdaptOptions::default()
    }
}

/// Median wall time of `runs` calls.
fn median(runs: usize, mut work: impl FnMut()) -> Duration {
    let mut samples: Vec<Duration> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            work();
            started.elapsed()
        })
        .collect();
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn ms(duration: Duration) -> String {
    format!("{:>9.3} ms", duration.as_secs_f64() * 1_000.0)
}

const SCREEN: Vec2 = Vec2::new(1440.0, 900.0);

fn raw_input(pointer: Pos2) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events: vec![egui::Event::PointerMoved(pointer)],
        ..egui::RawInput::default()
    }
}

/// Camera state for one benchmarked canvas.
struct Camera {
    pan: Vec2,
    zoom: f32,
    need_fit: bool,
}

/// One headless canvas frame with the pointer resting at `pointer`.
fn canvas_frame(
    ctx: &egui::Context,
    graph: &UiGraph,
    scene: &GraphScene,
    selected: Option<&str>,
    camera: &mut Camera,
    pointer: Pos2,
) {
    let output = ctx.run_ui(raw_input(pointer), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            draw_canvas(
                ui,
                graph,
                scene,
                selected,
                &mut camera.pan,
                &mut camera.zoom,
                &mut camera.need_fit,
            );
        });
    });
    std::hint::black_box(output.shapes.len());
}

fn detail_frame(ctx: &egui::Context, graph: &UiGraph, scene: &GraphScene, selected: &str) {
    let output = ctx.run_ui(raw_input(Pos2::new(10.0, 10.0)), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            draw_detail(
                ui,
                graph,
                scene,
                selected,
                GraphLens::Overview,
                DetailContentState {
                    body: None,
                    error: None,
                    loading: false,
                    available: true,
                },
            );
        });
    });
    std::hint::black_box(output.shapes.len());
}

#[test]
fn synthetic_view_has_the_requested_shape() {
    let view = synthetic_view(200, 800);
    assert_eq!((view.nodes.len(), view.edges.len()), (200, 800));
    let graph = adapt(&view, &overview_options());
    assert_eq!(graph.nodes.len(), 200);
    assert!(graph.edges.len() > 700, "few edges collapse");
    assert!(graph
        .edges
        .iter()
        .all(|edge| edge.source_id != edge.target_id));
}

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn graph_pipeline_benchmark() {
    let view = synthetic_view(NODES, EDGES);

    let adapt_time = median(15, || {
        std::hint::black_box(adapt(&view, &overview_options()));
    });
    let graph = adapt(&view, &overview_options());
    assert_eq!(graph.nodes.len(), NODES);

    let mut positions = PosCache::default();
    let grid_time = median(15, || overview_grid_place(&graph, &mut positions));
    let radial_time = median(15, || {
        let mut cache = PosCache::default();
        radial_place(&graph, Some("n00000"), &mut cache);
        std::hint::black_box(cache.len());
    });
    overview_grid_place(&graph, &mut positions);
    let scene_time = median(15, || {
        std::hint::black_box(GraphScene::build(&graph, &positions));
    });
    let scene = GraphScene::build(&graph, &positions);

    // Hub with the most incident edges: worst case for the detail panel.
    let hub = graph
        .nodes
        .iter()
        .max_by_key(|node| node.degree)
        .map(|node| node.id.clone())
        .expect("non-empty graph");

    let ctx = egui::Context::default();
    let center = (SCREEN / 2.0).to_pos2();

    // Fitted overview: every node and edge is on screen.
    let mut fitted = Camera {
        pan: Vec2::ZERO,
        zoom: 1.0,
        need_fit: true,
    };
    for _ in 0..3 {
        canvas_frame(&ctx, &graph, &scene, Some(&hub), &mut fitted, center);
    }
    let fitted_zoom = fitted.zoom;
    let frame_fitted = median(25, || {
        canvas_frame(&ctx, &graph, &scene, Some(&hub), &mut fitted, center);
    });

    // Zoomed in 8x around the centre: most of the graph is culled.
    let mut zoomed = Camera {
        pan: fitted.pan * 8.0 - center.to_vec2() * 7.0,
        zoom: fitted_zoom * 8.0,
        need_fit: false,
    };
    for _ in 0..3 {
        canvas_frame(&ctx, &graph, &scene, Some(&hub), &mut zoomed, center);
    }
    let frame_zoomed = median(25, || {
        canvas_frame(&ctx, &graph, &scene, Some(&hub), &mut zoomed, center);
    });

    let detail_ctx = egui::Context::default();
    for _ in 0..3 {
        detail_frame(&detail_ctx, &graph, &scene, &hub);
    }
    let detail_time = median(25, || detail_frame(&detail_ctx, &graph, &scene, &hub));

    // Expand used to deep-copy the local and the full view into the worker
    // command; both are now shared through `Arc`.
    let deep_copy = median(15, || {
        std::hint::black_box((view.clone(), Some(view.clone())));
    });
    let shared_view = std::sync::Arc::new(view.clone());
    let shared_copy = median(15, || {
        std::hint::black_box((
            std::sync::Arc::clone(&shared_view),
            Some(std::sync::Arc::clone(&shared_view)),
        ));
    });

    println!();
    println!(
        "graph pipeline benchmark: {} nodes / {} edges ({} drawn edges)",
        NODES,
        EDGES,
        graph.edges.len()
    );
    println!("  adapt()                         {}", ms(adapt_time));
    println!("  layout: overview grid           {}", ms(grid_time));
    println!("  layout: radial from seed        {}", ms(radial_time));
    println!("  scene build (per graph change)  {}", ms(scene_time));
    println!("  canvas frame, fitted overview   {}", ms(frame_fitted));
    println!("  canvas frame, zoomed in 8x      {}", ms(frame_zoomed));
    println!("  detail panel frame (hub node)   {}", ms(detail_time));
    println!("  expand payload, deep copy x2    {}", ms(deep_copy));
    println!("  expand payload, Arc x2          {}", ms(shared_copy));
}
