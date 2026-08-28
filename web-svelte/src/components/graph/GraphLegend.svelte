<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  import { legendEntries } from '@/lib/forceLayout'

  const entries = $derived(legendEntries(graph.nodes))
</script>

<div class="legend">
  {#each entries as e (`${e.source}:${e.label}`)}
    <div>
      <i style="background: {e.color}; color: {e.color}"></i>
      {e.label}
    </div>
  {/each}
  <div class="edge">- wikilink</div>
</div>

<style>
  .legend {
    position: absolute;
    left: 12px;
    bottom: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    border-radius: 12px;
    background: var(--graph-panel-bg);
    border: 1px solid var(--graph-panel-border);
    font-size: 12px;
    color: var(--graph-panel-text);
    backdrop-filter: blur(8px);
    pointer-events: none;
    max-height: min(40vh, 280px);
    overflow-y: auto;
  }
  i {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    margin-right: 8px;
    box-shadow: 0 0 10px currentColor;
    vertical-align: middle;
  }
  .edge {
    color: var(--graph-node-wiki);
    opacity: 0.9;
  }
</style>
