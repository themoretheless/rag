<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  const kinds = $derived(['wiki','document','tag','entity','stub'].map(kind => ({kind,count:graph.nodes.filter(n => (n.kind || 'document').toLowerCase().includes(kind)).length})).filter(item => item.count))
</script>
<div class="legend">
  {#each kinds as item}<div><i class={item.kind}></i><span>{item.kind}</span><b>{item.count}</b></div>{/each}
  <hr/>
  <div><i class="line wikilink"></i><span>wikilink</span></div><div><i class="line related"></i><span>related · compiled from</span></div><div><i class="line tagged"></i><span>tagged</span></div><div><i class="line tunnel"></i><span>tunnel · между крыльями</span></div>
</div>
<style>
 .legend{position:absolute;left:14px;bottom:14px;display:grid;gap:6px;padding:10px 12px;border-radius:11px;background:var(--graph-panel-bg);border:1px solid var(--graph-panel-border);font-size:11.5px;color:var(--graph-panel-text);backdrop-filter:blur(8px);pointer-events:none}.legend>div{display:grid;grid-template-columns:14px 1fr auto;align-items:center;gap:7px}.legend b{font:10.5px var(--mono);color:var(--graph-panel-muted)}.legend i{width:10px;height:10px;border-radius:50%;background:var(--graph-node-doc);box-shadow:0 0 8px currentColor}.legend i.wiki{background:var(--graph-node-wiki)}.legend i.tag{background:var(--graph-node-tag)}.legend i.entity{background:var(--graph-node-entity)}.legend i.stub{background:transparent;border:1px dashed var(--graph-node-stub)}hr{width:100%;height:1px;border:0;background:var(--graph-panel-border);margin:2px 0}.legend i.line{width:18px;height:0;border-radius:0;box-shadow:none;background:transparent;border-top:1px solid var(--graph-edge)}.legend i.wikilink{border-color:var(--graph-edge-wikilink)}.legend i.tagged{border-top-style:dotted;border-color:var(--graph-node-tag)}.legend i.tunnel{border-top:2px dashed var(--graph-node-entity)}
</style>
