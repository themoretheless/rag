<script lang="ts">
  import { api } from '@/api/client'
  import type { SearchHit } from '@/api/types'
  let { search, hits, disabled = false }: { search: Record<string, unknown>; hits: SearchHit[]; disabled?: boolean } = $props()
  let expected = $state<string[]>([])
  let other = $state('')
  let noAnswer = $state(false)
  let busy = $state(false)
  let message = $state('')
  let saved = $state(false)
  const id = crypto.randomUUID()
  const documents = $derived([...new Map(hits.map(hit => [hit.document_id, hit])).values()])
  async function save() {
    if (busy || disabled || saved) return
    busy = true; message = ''
    try {
      const sources = [...expected, ...other.split('\n').map(v => v.trim()).filter(Boolean)]
      await api.post('/v1/eval/feedback', { id, search, expected:noAnswer ? [] : sources, no_answer:noAnswer })
      saved = true; message = 'Вопрос и разметка сохранены. Повторный прогон доступен в разделе «Оценка».'
    } catch(e) { message = String(e) }
    finally { busy = false }
  }
</script>
<details class="assessment">
  <summary>Сохранить этот вопрос для оценки поиска</summary>
  <p>Отметьте проверенные источники. Текст вопроса и настройки сохранятся на сервере только после нажатия кнопки.</p>
  {#each documents as doc (doc.document_id)}<label><input type="checkbox" bind:group={expected} value={doc.document_id} disabled={disabled || busy || saved || noAnswer} />{doc.document_title}</label>{/each}
  <label>Другие правильные источники — URI или ID, по одному на строку<textarea bind:value={other} rows="2" disabled={disabled || busy || saved || noAnswer}></textarea></label>
  <label><input type="checkbox" bind:checked={noAnswer} disabled={disabled || busy || saved} /> В корпусе нет подтверждённого ответа на этот вопрос</label>
  <button onclick={save} disabled={disabled || busy || saved || (!noAnswer && !expected.length && !other.trim())}>{busy ? 'Сохраняем…' : saved ? 'Сохранено' : 'Сохранить вопрос и разметку'}</button>
  {#if message}<p role="status">{message}</p>{/if}
</details>
<style>
  .assessment { border:1px solid var(--border); border-radius:8px; padding:12px; margin:8px 0; max-height:300px; overflow:auto; flex-shrink:0; }
  summary { cursor:pointer; } p,label { font-size:12px; line-height:1.6; } label { display:block; margin:6px 0; }
  textarea { display:block; width:100%; box-sizing:border-box; background:var(--surface); color:var(--text); border:1px solid var(--border); }
  button { padding:8px; border:1px solid var(--border); border-radius:6px; background:var(--surface); color:var(--text); cursor:pointer; }
</style>
