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
| 4. Uniform grid for node culling and hit-testing | 72.7 | 1.7 | 11.0 | 11.2 | 41.2 | 4.4 | 1.7 |
| 5. `Arc<GraphView>` for the full and local views (Expand payload: 2 deep copies → 2 `Arc` clones, see below) | – | – | – | – | – | – | – |
| 6. `adapt()`: placement JSON parsed once per node, no edge double-clone, index-based depth/degree | 45.2 | 1.4 | 10.6 | 10.8 | 37.6 | 3.5 | 1.5 |
| 7. Search keys built on load (activity feed, seed picker); project list borrowed | 41.3 | 1.3 | 9.6 | 9.9 | 38.0 | 3.4 | 1.5 |

Step 4 is within noise at this size: the per-node loop was already a small
part of the frame, so the grid mostly bounds the cost for larger graphs. Step 7
does not touch the graph rows; its own measurements are below.

## Other measurements (final commit)

| What | Before | After |
| --- | ---: | ---: |
| Expand command payload (local view + full view) | 9.1 ms (two deep copies) | < 0.001 ms (two `Arc` clones) |
| Seed picker query that matches nothing (scans all 5 000 nodes) | 0.32 ms (lowercases three fields per node per query) | 0.14 ms, plus 0.57 ms once per loaded graph for the keys |

Not benchmarked, because the inputs are small (the activity feed holds tens of
events, the project list a handful): activity filter keys are lowercased when
the feed is loaded instead of three allocations per event per frame — the
combined search string used to be built even with an empty filter — and the
top bar borrows the project list instead of cloning it every frame.

## Summary

| Row | Baseline | Final | Change |
| --- | ---: | ---: | ---: |
| `adapt()` | 83.8 ms | 41.3 ms | −51 % |
| layout: overview grid | 1.9 ms | 1.3 ms | noise |
| layout: radial from seed | 12.1 ms | 9.6 ms | noise (untouched) |
| scene build (once per graph or layout change) | – | 9.9 ms | new |
| canvas frame, fitted overview | 45.2 ms | 38.0 ms | −16 % |
| canvas frame, zoomed in 8x | 148.3 ms | 3.4 ms | −98 % |
| detail panel frame (hub node) | 3.6 ms | 1.5 ms | −58 % |

The scene build is paid when the graph or its layout changes (load, lens or
filter change, expand), not per frame. Most of it is sorting each node's
incident edges for the detail panel; it could be made lazy per selected node
if it ever shows up.

## What still costs time

The fitted overview frame is the remaining hot spot, and what is left in it
is drawing, not bookkeeping:

- With everything on screen each dashed edge still emits one line shape per
  dash (roughly 4 000 dashed edges here, tens of dashes each). Merging them
  into a single mesh would be much cheaper but changes anti-aliasing, so it
  was left out of a behaviour-preserving pass.
- Below the dense-overview threshold (more than 5 000 nodes) every visible
  node is a focusable egui widget with an AccessKit label; at exactly 5 000
  nodes that is 5 000 `ui.interact` calls per frame. This is the keyboard and
  screen-reader contract of the canvas and was not touched.

`panic_fallback` is still built before each worker job rather than only after
a panic: the job consumes the command, so deferring it would mean cloning the
command or making `run` borrow it, and either costs more than the few short
strings the fallback copies.
