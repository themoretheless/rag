# egui graph view: performance baseline

Stage 6 of the refactor plan (native `rag-mcp-ui` client). Numbers for the
graph pipeline before and after each optimisation, all from the same
benchmark.

## How it is measured

`crates/rag-mcp-ui/src/bench.rs` builds a deterministic synthetic graph of
5 000 nodes and 20 000 edges (documents across two layers and seven projects,
tags, stubs, entities; ten relation types; 16 hub nodes) and times:

| Row | What runs |
| --- | --- |
| `adapt()` | `GraphView` → `UiGraph` (filters, multi-edge collapse, caps, depth/degree) |
| layout: overview grid | `overview_grid_place` for all 5 000 nodes |
| layout: radial from seed | `radial_place` from one seed |
| canvas frame, fitted overview | one `draw_canvas` call, whole graph on screen, pointer over the canvas, one node selected |
| canvas frame, zoomed in 8x | the same with the camera zoomed 8x on the centre (most nodes off screen) |
| detail panel frame (hub node) | one `draw_detail` call for the node with the most incident edges |

The frame rows use a headless `egui::Context` (`run_ui`), so they cover what
the client does on the CPU every frame — culling, hit-testing, legend and
tooltip data, incident-edge list and shape generation — but not tessellation
or the GPU. Each row is the median of 15 (pipeline) or 25 (frame) runs after
three warm-up frames.

```
cargo test -p rag-mcp-ui --no-default-features --release --locked -- \
    --ignored --nocapture graph_pipeline_benchmark
```

Machine: 4 vCPU Intel Xeon @ 2.10 GHz, Linux, rustc 1.99.1, release profile.
The machine was shared with other builds while measuring (load average around
10), so single numbers move by 10–15 % between runs; each cell below is the
lower of two consecutive runs. Compare rows, not digits.

## Baseline

Commit `b2d5cca` (after the structural refactor, before any Stage 6
optimisation).

| Row | Time |
| --- | ---: |
| `adapt()` | 83.8 ms |
| layout: overview grid | 1.9 ms |
| layout: radial from seed | 12.1 ms |
| canvas frame, fitted overview | 45.2 ms |
| canvas frame, zoomed in 8x | 148.3 ms |
| detail panel frame (hub node) | 3.6 ms |

Observations that set the order of work:

- The zoomed-in frame is three times slower than the fitted one although most
  nodes are culled. Dashed edges (`tagged`, `mentions`) are drawn dash by dash
  along their whole length, including the part outside the viewport, and that
  length grows with zoom.
- Every edge costs two `HashMap<String, Pos2>` lookups per frame, and the
  legend, the hover tooltip and the hit-test each rescan all nodes or edges.

## Results per step

Times in ms, lower of two runs. "scene" is the per-graph draw data
(`GraphScene`) that is now built once when the graph or its layout changes
instead of being recomputed inside every frame.

| Step | adapt | grid | radial | scene build | frame fitted | frame zoomed | detail |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline | 83.8 | 1.9 | 12.1 | – | 45.2 | 148.3 | 3.6 |
| 1. Positions and edge endpoints by node index | 87.2 | 1.9 | 12.2 | 0.15 | 40.1 | 140.5 | 3.4 |
| 2. Edge shapes in one painter call, dashes clipped to the viewport, unit-circle table | 71.9 | 1.8 | 11.5 | 0.14 | 44.2 | 5.9 | 3.4 |
| 3. Legend, hover tooltip data and incident-edge lists cached in the scene | 73.8 | 1.9 | 12.5 | 12.3 | 40.5 | 4.1 | 1.4 |
