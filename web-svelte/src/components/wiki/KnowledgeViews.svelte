<script lang="ts">
  import { api } from '@/api/client'
  import { goWiki } from '@/lib/router.svelte'
  import { wiki } from '@/lib/state/wiki.svelte'
  type Props = {document_type:string;review_status:string;owner:string;review_due:string}
  type Filter = {q:string;wing:string;document_type:string;review_status:string;owner:string;due_before:string;offset:number}
  type Row = {id:string;uri:string;title:string;wing:string|null;revision:number;properties:Partial<Props>}
  type View = {id:string;name:string;revision:number;filters:Filter}
  const blankProps:Props={document_type:'',review_status:'',owner:'',review_due:''}
  const blankFilter:Filter={q:'',wing:'',document_type:'',review_status:'',owner:'',due_before:'',offset:0}
  let filter = $state<Filter>({...blankFilter})
  let applied = $state<Filter>({...blankFilter})
  let rows = $state<Row[]>([])
  let views = $state<View[]>([])
  let view = $state<View|null>(null)
  let name = $state('')
  let total = $state(0)
  let busy = $state(false)
  let loaded = $state(false)
  let message = $state('')
  let selected = $state<Row|null>(null)
  let properties = $state<Props>({...blankProps})
  const dirty = $derived(!!selected && JSON.stringify(properties)!==JSON.stringify({...blankProps,...selected.properties}))
  async function refresh(more=false) {
    if (busy || dirty) return
    busy=true;message=''
    const next = more?{...applied,offset:rows.length}:{...filter,offset:0}
    try {
      const qs=new URLSearchParams(Object.entries(next).map(([k,v])=>[k,String(v)]))
      const result=await api.get<{items:Row[];total:number}>(`/v1/knowledge?${qs}`)
      rows=more?[...rows,...result.items]:result.items;total=result.total;applied=next;loaded=true;selected=null
    } catch(e){message=String(e)} finally{busy=false}
  }
  async function loadViews() {
    if(busy || dirty)return
    busy=true;message=''
    try{views=(await api.get<{items:View[]}>('/v1/knowledge/views')).items}catch(e){message=String(e)}finally{busy=false}
  }
  function choose(v:View) {view=v;name=v.name;filter={...v.filters};void refresh()}
  async function saveView(copy=false) {
    if(busy)return
    busy=true;message=''
    try {
      const result=await api.post<View>('/v1/knowledge/views',{id:copy?undefined:view?.id,revision:copy?undefined:view?.revision,name,filters:{...filter,offset:0}})
      views=[result,...views.filter(v=>v.id!==result.id)];view=result;message='Представление сохранено.'
    }catch(e){message=String(e)}finally{busy=false}
  }
  function edit(row:Row){selected=row;properties={...blankProps,...row.properties}}
  async function saveProperties() {
    if(!selected || busy)return
    busy=true;message=''
    try {
      const result=await api.put<{revision:number}>('/v1/knowledge',{id:selected.id,revision:selected.revision,properties})
      selected={...selected,revision:result.revision,properties:{...properties}}
      rows=rows.map(row=>row.id===selected?.id?selected:row) as Row[]
      wiki.pages=wiki.pages.map(page=>page.id===selected?.id?{...page,revision:result.revision}:page)
      message='Свойства сохранены. Примените фильтры повторно, чтобы обновить состав представления.'
    }catch(e){message=String(e).includes('HTTP 409')?'Статья изменилась. Ваши правки остаются в форме; скопируйте их, отмените форму и загрузите свежую версию.':String(e)}finally{busy=false}
  }
</script>
<details class="knowledge"><summary>Свойства и сохранённые представления</summary>
<p>Представления отбирают актуальный список wiki-статей по свойствам. Статус «Проверено» задаёт человек; он не заменяет проверку источников.</p>
<div class="filters">
<label>Название<input bind:value={filter.q} /></label><label>Проект<input bind:value={filter.wing} /></label>
<label>Тип<select bind:value={filter.document_type}><option value="">Любой</option><option value="note">Заметка</option><option value="decision">Решение</option><option value="guide">Инструкция</option><option value="service">Сервис</option><option value="research">Исследование</option></select></label>
<label>Проверка<select bind:value={filter.review_status}><option value="">Любой статус</option><option value="draft">Черновик</option><option value="needs_review">Нужна проверка</option><option value="verified">Проверено</option></select></label>
<label>Ответственный<input bind:value={filter.owner} /></label><label>Пересмотреть до<input type="date" bind:value={filter.due_before}/></label>
</div>
<button disabled={busy || dirty} onclick={()=>refresh()}>Применить фильтры</button>
<button disabled={busy || dirty} onclick={loadViews}>Загрузить представления</button>
{#each views as v (v.id)}<button disabled={busy || dirty} onclick={()=>choose(v)}>{v.name}</button>{/each}
<div class="save"><input aria-label="Название представления" bind:value={name} placeholder="Название представления"/><button disabled={busy || !name.trim()} onclick={()=>saveView()}>{view?'Обновить представление':'Сохранить представление'}</button>{#if view}<button disabled={busy || !name.trim()} onclick={()=>saveView(true)}>Сохранить копию</button>{/if}</div>
{#if message}<p role="status">{message}</p>{/if}
{#if loaded}<p>{total} статей · показано {rows.length}</p>{/if}
{#each rows as row (row.id)}<article><button disabled={busy || dirty} onclick={()=>goWiki(row.id)}>{row.title}</button><span>{row.wing || 'Без проекта'} · {row.properties.owner || 'Без ответственного'} · {row.properties.review_due || 'Без даты'}</span><button disabled={busy || dirty} onclick={()=>edit(row)}>Свойства</button></article>{/each}
{#if rows.length<total}<button disabled={busy || dirty} onclick={()=>refresh(true)}>Загрузить ещё</button>{/if}
{#if selected}<section aria-label="Свойства статьи"><h3>{selected.title}</h3><div class="filters">
<label>Тип статьи<select bind:value={properties.document_type} disabled={busy}><option value="">Не задан</option><option value="note">Заметка</option><option value="decision">Решение</option><option value="guide">Инструкция</option><option value="service">Сервис</option><option value="research">Исследование</option></select></label>
<label>Статус проверки<select bind:value={properties.review_status} disabled={busy}><option value="">Не задан</option><option value="draft">Черновик</option><option value="needs_review">Нужна проверка</option><option value="verified">Проверено</option></select></label>
<label>Ответственный за статью<input bind:value={properties.owner} disabled={busy}/></label><label>Дата пересмотра<input type="date" bind:value={properties.review_due} disabled={busy}/></label></div>
<button disabled={busy || !dirty} onclick={saveProperties}>Сохранить свойства</button><button disabled={busy} onclick={()=>{selected=null}}>Отменить</button></section>{/if}
</details>
<style>
.knowledge{border:1px solid var(--border);border-radius:12px;padding:16px;margin-bottom:24px} summary{cursor:pointer} .filters{display:flex;flex-wrap:wrap;gap:12px} label{font-size:12px;line-height:1.6} input,select{display:block;background:var(--surface);color:var(--text);padding:7px;border:1px solid var(--border);border-radius:5px;max-width:220px} button{padding:7px 10px;margin:6px 4px 6px 0;background:var(--surface);color:var(--text);border:1px solid var(--border);border-radius:6px;cursor:pointer} button:disabled{opacity:.5} p,span{font-size:12px;line-height:1.5} article{display:flex;align-items:center;justify-content:space-between;gap:12px;border-top:1px solid var(--border)} article span{overflow-wrap:anywhere}.save{display:flex;flex-wrap:wrap;align-items:center;gap:8px} section{padding:12px;border:1px solid var(--border)}
</style>
