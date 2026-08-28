<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import type { LayoutMode } from '@/lib/forceLayout'

  let {
    onexpand,
    onfull,
    onfit,
  }: {
    onexpand: () => void
    onfull: () => void
    onfit: () => void
  } = $props()

  function pickLayout(mode: LayoutMode) {
    // Clicking the already-effective layout clears the override (back to auto).
    graph.setLayout(graph.layoutOverride === mode ? null : mode)
  }
</script>

<div class="tb">
  <div class="left">
    <strong>{ui.t('graph')}</strong>
    <span class="chip">{ui.t('graphNodes', { count: graph.nodes.length })}</span>
    <span class="chip">{ui.t('graphEdges', { count: graph.edges.length })}</span>
    <span class="chip">{graph.mode}</span>
    <div class="seg" role="group" aria-label="layout">
      <button
        type="button"
        class:active={graph.layout === 'force'}
        onclick={() => pickLayout('force')}
      >
        {ui.t('layoutForce')}
      </button>
      <button
        type="button"
        class:active={graph.layout === 'radial'}
        onclick={() => pickLayout('radial')}
      >
        {ui.t('layoutRadial')}
      </button>
    </div>
    <button
      type="button"
      class:active={graph.focusMode}
      title={ui.t('graphFocus')}
      onclick={() => graph.toggleFocusMode()}
    >
      ◎
    </button>
    <button type="button" title={ui.t('graphFit')} onclick={onfit}>⛶</button>
  </div>
  <div class="mid">
    <input bind:value={graph.seed} type="text" placeholder={ui.t('graphSeedPlaceholder')} />
    <label>
      {ui.t('graphDepth')}
      <input bind:value={graph.depth} type="number" min="1" max="3" />
    </label>
    <label>
      {ui.t('graphMax')}
      <input bind:value={graph.maxNodes} type="number" min="20" max="2000" step="20" />
    </label>
    <label class="check">
      <input bind:checked={graph.includeTags} type="checkbox" />
      {ui.t('graphTags')}
    </label>
  </div>
  <div class="right">
    <button type="button" onclick={onfull}>{ui.t('graphFull')}</button>
    <button type="button" class="accent" onclick={onexpand}>{ui.t('graphExpand')}</button>
  </div>
</div>

<style>
  .tb {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-bottom: 1px solid var(--graph-panel-border);
    background: var(--graph-panel-bg);
    color: var(--graph-panel-text);
    backdrop-filter: blur(8px);
  }
  .left,
  .mid,
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .chip {
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 999px;
    background: var(--graph-panel-chip);
    color: var(--graph-panel-muted);
  }
  .seg {
    display: flex;
    border: 1px solid var(--graph-panel-border);
    border-radius: 8px;
    overflow: hidden;
  }
  .seg button {
    border: none;
    border-radius: 0;
    padding: 6px 10px;
    font-size: 12px;
  }
  .seg button.active {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  input[type='text'],
  input[type='number'] {
    background: var(--graph-panel-input);
    border: 1px solid var(--graph-panel-border);
    border-radius: 8px;
    padding: 6px 10px;
    color: var(--graph-panel-text);
    outline: none;
  }
  input[type='text'] {
    min-width: 220px;
  }
  input[type='number'] {
    width: 64px;
  }
  label {
    font-size: 12px;
    color: var(--graph-panel-muted);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  button {
    border: 1px solid var(--graph-panel-border);
    background: var(--graph-panel-chip);
    color: var(--graph-panel-text);
    border-radius: 8px;
    padding: 7px 12px;
    cursor: pointer;
  }
  button.active {
    background: var(--accent-soft);
    color: var(--accent);
    border-color: transparent;
  }
  button.accent {
    background: linear-gradient(135deg, var(--graph-node-wiki), var(--accent));
    border: none;
    color: #fff;
    font-weight: 600;
  }
  button:hover {
    filter: brightness(1.08);
  }
</style>
