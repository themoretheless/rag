<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  import { api } from '@/api/client'
  import { goWiki, navigate } from '@/lib/router.svelte'

  let { onexpand }: { onexpand?: (seedKey:string)=>void } = $props()
  let detail = $state<any>(null)
  const node = $derived(graph.nodes.find(item => item.id === graph.selectedId) || null)
  const wikiOpenable = $derived(Boolean(node?.document_id && (node?.layer === 'wiki' || node?.uri?.startsWith('wiki://') || /^wiki$/i.test(node?.kind ?? ''))))
  const degree = $derived(node ? graph.edges.filter(edge => edge.source_id===node.id || edge.target_id===node.id).length : 0)
  const relations = $derived.by(()=>{const map=new Map<string,number>();if(node)for(const edge of graph.edges){if(edge.source_id===node.id||edge.target_id===node.id)map.set(edge.rel_type,(map.get(edge.rel_type)||0)+1)}return [...map.entries()]})
  const metadata = $derived.by(()=>{try{return JSON.parse(node?.metadata_json||'{}')}catch{return {}}})
  let requestId = 0
  $effect(()=>{const current=++requestId;const id=node?.document_id;detail=null;if(!id)return;api.document({id}).then(value=>{if(current===requestId)detail=value}).catch(()=>{})})
  function expand(){if(node)onexpand?.((node.document_id||node.label||node.id).trim())}
  function searchChunks(){if(node?.document_id)navigate(`/search?document_id=${encodeURIComponent(node.document_id)}&q=${encodeURIComponent(node.label)}`)}
  function openWiki(){if(wikiOpenable && node?.document_id)goWiki(node.document_id)}
</script>
{#if node}<aside class="insp"><button class="x" aria-label="Закрыть инспектор графа" onclick={()=>graph.select(null)}>×</button><span class={`kind ${node.kind}`}>{node.kind}</span><h3>{node.label}</h3><dl><dt>uri</dt><dd>{node.uri||detail?.uri||'—'}</dd><dt>node id</dt><dd>{node.id}</dd><dt>степень</dt><dd>{degree}</dd><dt>wing / room</dt><dd>{detail?.wing||metadata.wing||'—'} / {detail?.room||metadata.room||'—'}</dd><dt>revision</dt><dd>{detail?.revision!=null?`r${detail.revision}`:'—'}</dd></dl><div class="rels">{#each relations as [name,count]}<span>{name} · {count}</span>{/each}</div><div class="actions"><button onclick={expand}>Развернуть отсюда (depth {graph.depth})</button><button onclick={searchChunks} disabled={!node.document_id}>Показать чанки в поиске</button><button class="open" onclick={openWiki} disabled={!wikiOpenable}>Открыть в вики</button></div></aside>{/if}
<style>
 .insp{position:absolute;right:14px;top:66px;width:300px;padding:15px;border-radius:13px;background:var(--graph-panel-bg);border:1px solid var(--graph-panel-border);backdrop-filter:blur(12px);box-shadow:var(--graph-shadow);color:var(--graph-panel-text);z-index:7}.x{position:absolute;right:8px;top:6px;border:0;background:transparent;color:var(--graph-panel-muted);font-size:18px;cursor:pointer}.kind{font:500 10.5px var(--mono);padding:3px 6px;border-radius:5px;background:var(--graph-panel-chip);color:var(--graph-node-doc)}.kind.wiki{color:var(--graph-node-wiki)}.kind.tag{color:var(--graph-node-tag)}.kind.entity{color:var(--graph-node-entity)}h3{margin:8px 25px 10px 0;font-size:16px;line-height:1.25}dl{display:grid;grid-template-columns:76px 1fr;gap:6px 8px;margin:0 0 12px;font-size:11.5px}dt{color:var(--graph-panel-muted)}dd{margin:0;font:10.5px/1.4 var(--mono);word-break:break-all}.rels{display:flex;flex-wrap:wrap;gap:5px;margin-bottom:12px}.rels span{padding:3px 7px;border-radius:9px;background:var(--graph-panel-chip);font:10px var(--mono);color:var(--graph-panel-muted)}.actions{display:grid;gap:7px}.actions button{height:32px;border:1px solid var(--graph-panel-border);border-radius:7px;background:var(--graph-panel-chip);color:var(--graph-panel-text);font-size:12px;cursor:pointer}.actions button.open{border:0;background:linear-gradient(135deg,var(--graph-node-wiki),var(--l0));color:#08101b;font-weight:700}.actions button:disabled{opacity:.4}
</style>
