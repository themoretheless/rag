<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { goWiki } from '@/lib/router.svelte'

  let { onexpand }: { onexpand?: (seedKey: string) => void } = $props()

  const node = $derived(graph.nodes.find((n) => n.id === graph.selectedId) || null)

  const degree = $derived.by(() => {
    if (!node) return 0
    const id = node.id
    return graph.edges.filter((e) => e.source_id === id || e.target_id === id).length
  })

  function openWiki() {
    const docId = node?.document_id
    if (docId) goWiki(docId)
  }

  function expandHere() {
    if (!node) return
    onexpand?.((node.document_id || node.label || node.id).trim())
  }
</script>

{#if node}
  <aside class="insp">
    <button class="x" type="button" onclick={() => graph.select(null)}>×</button>
    <h3>{node.label}</h3>
    <dl>
      <dt>{ui.t('kind')}</dt>
      <dd>{node.kind || '—'}</dd>
      <dt>id</dt>
      <dd class="mono">{node.id}</dd>
      <dt>document</dt>
      <dd class="mono">{node.document_id || '—'}</dd>
      <dt>degree</dt>
      <dd>{degree}</dd>
      <dt>wing/room</dt>
      <dd>{node.wing || '—'} / {node.room || '—'}</dd>
    </dl>
    <div class="actions">
      <button type="button" class="ghost-btn" onclick={expandHere}>{ui.t('graphExpand')}</button>
      {#if node.document_id}
        <button type="button" class="open" onclick={openWiki}>{ui.t('graphOpenAsWiki')}</button>
      {/if}
    </div>
  </aside>
{/if}

<style>
  .insp {
    position: absolute;
    top: 12px;
    right: 12px;
    width: 280px;
    padding: 14px;
    border-radius: 14px;
    background: var(--graph-panel-bg);
    border: 1px solid var(--graph-panel-border);
    box-shadow: var(--graph-shadow);
    color: var(--graph-panel-text);
    backdrop-filter: blur(10px);
  }
  .x {
    position: absolute;
    right: 8px;
    top: 6px;
    border: none;
    background: transparent;
    color: var(--graph-panel-muted);
    font-size: 18px;
    cursor: pointer;
  }
  h3 {
    margin: 0 28px 10px 0;
    font-size: 16px;
    letter-spacing: -0.02em;
  }
  dl {
    display: grid;
    grid-template-columns: 72px 1fr;
    gap: 6px 8px;
    margin: 0 0 12px;
    font-size: 12px;
  }
  dt {
    color: var(--graph-panel-muted);
  }
  dd {
    margin: 0;
    word-break: break-all;
  }
  .mono {
    font-family: var(--mono);
    font-size: 11px;
  }
  .actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .open {
    width: 100%;
    border: none;
    border-radius: 8px;
    padding: 8px;
    background: linear-gradient(135deg, var(--graph-node-wiki), var(--accent));
    color: white;
    font-weight: 600;
    cursor: pointer;
  }
  .ghost-btn {
    width: 100%;
    border: 1px solid var(--graph-panel-border);
    border-radius: 8px;
    padding: 8px;
    background: var(--graph-panel-chip);
    color: var(--graph-panel-text);
    cursor: pointer;
  }
</style>
