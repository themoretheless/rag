<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import type { LayoutMode } from '@/lib/forceLayout'
  let { onexpand, onfull, onfit, onzoomin, onzoomout, onprojectchange, onreload }: { onexpand:()=>void; onfull:()=>void; onfit:()=>void; onzoomin:()=>void; onzoomout:()=>void; onprojectchange:()=>void; onreload:()=>void | Promise<void> } = $props()
  function pickLayout(mode:LayoutMode){graph.setLayout(graph.layoutOverride===mode?null:mode)}
  async function toggleTags(){
    const previous = graph.includeTags
    const requested = !previous
    graph.includeTags = requested
    await onreload()
    if (graph.error && graph.includeTags === requested) graph.includeTags = previous
  }
</script>
<div class="tb">
  <div class="left">
    <strong>{ui.t('graph')}</strong>
    <select class="project" value={graph.project} onchange={(event) => { graph.project = event.currentTarget.value; onprojectchange() }} title={ui.t('graphProject')} aria-label={ui.t('graphProject')}><option value="">{ui.t('graphAllProjects')}</option>{#each graph.projects as item}<option value={item.project_id}>{item.project_id} · {item.document_count}</option>{/each}</select>
    <span class="chip">{ui.t('graphNodes', { count: graph.nodes.length })}</span><span class="chip">{ui.t('graphEdges', { count: graph.edges.length })}</span>
    <div class="seg" role="group" aria-label={ui.t('graph')}><button class:active={graph.layout==='force'} aria-pressed={graph.layout==='force'} onclick={()=>pickLayout('force')}>{ui.t('layoutForce')}</button><button class:active={graph.layout==='radial'} aria-pressed={graph.layout==='radial'} onclick={()=>pickLayout('radial')}>{ui.t('layoutRadial')}</button></div>
    <button class:active={graph.focusMode} aria-pressed={graph.focusMode} aria-label={ui.t('graphFocus')} title={ui.t('graphFocus')} onclick={()=>graph.toggleFocusMode()}>{ui.t('graphFocusToggle')}</button>
    <button class:active={graph.includeTags} aria-pressed={graph.includeTags} disabled={graph.loading} onclick={toggleTags}>{ui.t('graphTagsToggle')}</button>
    <div class="zoom"><button aria-label={ui.t('graphZoomOut')} title={ui.t('graphZoomOut')} onclick={onzoomout}>−</button><button aria-label={ui.t('graphZoomIn')} title={ui.t('graphZoomIn')} onclick={onzoomin}>+</button><button aria-label={ui.t('graphFit')} title={ui.t('graphFit')} onclick={onfit}>⛶</button></div>
    <select bind:value={graph.labelMode} title={ui.t('graphLabelDensity')} aria-label={ui.t('graphLabelDensity')}><option value="hubs">{ui.t('graphLabelsHubs')}</option><option value="all">{ui.t('graphLabelsAll')}</option><option value="none">{ui.t('graphLabelsNone')}</option></select>
  </div>
  <div class="right"><input bind:value={graph.seed} placeholder={ui.t('graphSeedHint')} aria-label={ui.t('graphSeedLabel')}/><label>{ui.t('graphDepth')} <input bind:value={graph.depth} type="number" min="1" max="3"/></label><label>{ui.t('graphMax')} <input bind:value={graph.maxNodes} type="number" min="20" max="300"/></label><button onclick={onfull}>{ui.t('graphFull')}</button><button class="accent" onclick={onexpand}>{ui.t('graphExpand')}</button></div>
</div>
<style>
  .tb{min-height:45px;display:flex;align-items:center;justify-content:space-between;gap:10px;padding:8px 11px;border:1px solid var(--graph-panel-border);border-radius:11px;background:var(--graph-panel-bg);backdrop-filter:blur(10px);box-shadow:var(--graph-shadow);color:var(--graph-panel-text)}.left,.right{display:flex;align-items:center;gap:6px;min-width:0}.left>strong{font-size:13.5px;margin-right:2px}.chip{padding:3px 7px;border-radius:10px;background:var(--graph-panel-chip);color:var(--graph-panel-muted);font:10.5px var(--mono);white-space:nowrap}.tb button,.tb select,.tb input{height:26px;border:1px solid var(--graph-panel-border);border-radius:7px;background:var(--graph-panel-chip);color:var(--graph-panel-text);font-size:11.5px}.tb button{padding:0 8px;cursor:pointer}.tb button.active{background:color-mix(in srgb,var(--l2) 14%,transparent);color:var(--l2)}.seg,.zoom{display:flex;border:1px solid var(--graph-panel-border);border-radius:7px;overflow:hidden}.seg button,.zoom button{border:0;border-right:1px solid var(--graph-panel-border);border-radius:0}.zoom button{width:26px;padding:0}.tb select{padding:0 5px;color:var(--graph-panel-muted);max-width:112px}.tb select.project{max-width:128px;color:var(--l2)}.right>input{width:155px;padding:0 8px;font:10.5px var(--mono)}label{display:flex;align-items:center;gap:4px;color:var(--graph-panel-muted);font-size:10.5px;white-space:nowrap}label input{width:42px;padding:0 4px;font:10.5px var(--mono)}.tb button.accent{border:0;background:linear-gradient(135deg,var(--l3),var(--l0));color:#08101b;font-weight:700}@media(max-width:1200px){.left .chip,.left select:not(.project),.right label{display:none}.right>input{width:130px}}
</style>
