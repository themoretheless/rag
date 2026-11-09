# Moving the rag clients to Forma

Status: analysis done, no code yet. Source: a read-only study of
`themoretheless/forma` at `f2b0afc` and of both rag clients at `a17c6be`.

Decisions from the owner:

- Both clients (egui desktop and Svelte web) move to Forma.
- The egui and Svelte refactors already in progress (Stages 6 and 7) finish first.
- What Forma lacks is added in the Forma repo, in its own pull requests.

## What Forma is today

One window holds one `Runtime` with a single root Frame, at most one root
`Scroll`, optional `Modal` layers and a flat list of leaf controls. Every model
change re-serializes and re-parses the whole scene.

Usable now: single-field text editing (graphemes, undo, IME, clipboard, wrap),
modal dialogs, typed Rust bindings, an AccessKit bridge, `native_window::run`,
and offscreen painting through `Runtime::paint_rgba`.

Missing for rag:

- independent scroll panes and layout that follows the window size;
- more than about 256 controls per scene, and updates without a full rebuild;
- any app-drawn surface (the graph: 5000 nodes, 20000 edges);
- styled text: weights, monospace, inline links, markdown;
- non-modal popups, menus, tooltips;
- a thread-safe way for a worker to wake the window;
- typed forms without Node.js and a sibling tokenizer checkout;
- a packaged web host; the WASM build is 21.8 MB (13.2 MB gzipped).

## Forma additions

Ordered by how much of rag each one unblocks.

| Id | Addition | Unblocks | Size |
|---|---|---|---|
| A10 | Push app state into a `from_ui_files` runtime (no Node at build time) | everything | S |
| A13 | Release tag, wasm-only `wasm-bindgen`, optional `svet-wasm`, `rust-version` | using Forma as a git dependency | S–M |
| A3 | Production host: no backend chooser, no F-key capture, plain title, `Send` waker, key hook, resize callback, close veto, right and double click | every native screen | S–M |
| A1 | Several scroll viewports and window-sized root | almost every screen | M–L |
| A2 | Windowed `for` and a higher control budget | Library, Search, Wiki list, Revisions, Activity, Corpus, Agents | M–L |
| A4 | Anchored popups, menus, select, tooltips, toasts | shell, Library, Search, Activity | M |
| A5 | Font weights, monospace, styled runs with link events | previews, snippets, diff, wiki | M–L |
| A6 | Markdown view, content taller than 4096 px | wiki read, document preview | L |
| A7 | Canvas control with display-list injection, pan and zoom, app hit testing | graph, sync diagram, charts | M–L |
| A8 | SVG `<path>` in icons | navigation and toolbars | S |
| A12 | TextInput above 100 KB | wiki editing | S–M |
| A14 | Packaged web host using the Rust compiler | every web screen | M |
| A11 | Mermaid | wiki diagrams (code block as fallback) | L, deferred |

A1 and A2 also need changes in `themoretheless/tokenizer`, where the scene
parser, the 256-control cap and the single-scroll rule live.

## Order of work

1. **Forma, gating:** A10, A13, A3, A8.
2. **rag, UI-free client crate:** move the HTTP layer, loaders, search, operations,
   revisions, worker, graph sampling and layout out of `rag-mcp-ui` so both the
   egui and the Forma front ends use them. Wire types stay in `rag-mcp-core`.
3. **Forma islands inside the egui app:** each migrated screen is an offscreen
   `Runtime` painted with `paint_rgba` into an egui texture, with input forwarded.
   Old and new screens run side by side. Order: connect screen, close and prune
   dialogs, operations, home, models and evaluation, then activity (A2), search
   and library (A2, A4, A5), revisions (A5), wiki edit (A12), wiki read (A6),
   sync diagram and charts (A7), graph (A7).
4. **Flip the host:** after A1 and the graph, the app starts through
   `native_window::run` with a Forma shell; eframe is removed.
5. **Web:** after A14. The same `.ui` sources run in the browser through the
   WASM build. WASM size is the open risk; Svelte stays until parity.

## Risks

- Forma pins rustc 1.97.1; rag keeps an MSRV 1.95 check, which the UI crate
  will have to leave.
- Forma pins `wasm-bindgen = "=0.2.125"` on every target; rag resolves 0.2.126.
- Two wgpu majors plus Forma's own GPU crates during coexistence.
- Native Forma is qualified on macOS only; Linux runs in CI on software Vulkan.
- About 18.6 MB of embedded fonts in every binary.
- Forma has no release tags and an experimental API.
