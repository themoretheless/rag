<script lang="ts">
  import { api } from '@/api/client'
  type Question = {id:string;search:{query:string;mode:string};expected:{uri:string}[];no_answer:boolean}
  let items = $state<Question[]>([])
  let total = $state(0)
  let loaded = $state(false)
  let busy = $state(false)
  let message = $state('')
  let results = $state<Record<string, {status:string;recall?:number;mrr?:number;empty_result_for_no_answer?:boolean}>>({})
  async function load(more=false) {
    if(busy) return
    busy=true;message=''
    try {const result=await api.get<{items:Question[];total:number}>(`/v1/eval/feedback?offset=${more?items.length:0}`);items=more?[...items,...result.items]:result.items;total=result.total;loaded=true}
    catch(e){message=String(e)} finally{busy=false}
  }
  async function run(id:string) {
    busy=true;message=''
    try {results[id]=await api.post('/v1/eval/feedback/run',{id})} catch(e){message=String(e)} finally{busy=false}
  }
</script>
<details class="real-eval"><summary>Реальные вопросы и ручная разметка</summary>
<p>Вопросы сохраняются из поиска по явному действию. Прогон использует текущий корпус и исходные настройки; изменённые контрольные источники требуют повторной разметки.</p>
<button disabled={busy} onclick={()=>load()}>Загрузить вопросы</button>
{#if message}<p role="alert">{message}</p>{/if}
{#if loaded && !items.length}<p>Сохранённых вопросов пока нет. Выполните поиск и отметьте правильные источники.</p>{/if}
{#each items as item (item.id)}<article><strong>{item.search.query}</strong><p>{item.search.mode} · {item.no_answer?'Нет ответа в корпусе':item.expected.map(e=>e.uri).join(', ')}</p><button disabled={busy} onclick={()=>run(item.id)}>Проверить сейчас</button>
{#if results[item.id]}{@const result=results[item.id]}<p>{result.status==='source_changed'?'Контрольный источник изменился — нужна новая разметка.':item.no_answer?`Пустая выдача: ${result.empty_result_for_no_answer?'да':'нет'}. Это не оценка правильности ответа модели.`:`Recall: ${result.recall?.toFixed(3)} · MRR: ${result.mrr?.toFixed(3)}`}</p>{/if}</article>{/each}
{#if items.length<total}<button disabled={busy} onclick={()=>load(true)}>Загрузить ещё</button>{/if}
</details>
<style>
.real-eval {padding:12px;border:1px solid var(--border);border-radius:8px;max-height:400px;overflow:auto;flex-shrink:0} summary{cursor:pointer} p{font-size:12px;line-height:1.5;overflow-wrap:anywhere} article{padding:12px 0;border-top:1px solid var(--border)} button{padding:7px 12px;background:var(--surface);color:var(--text);border:1px solid var(--border);border-radius:6px;cursor:pointer}
</style>
