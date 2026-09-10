<script lang="ts">
  import { api, apiUrl, type WikiProposal } from '@/api/client'
  import { wiki } from '@/lib/state/wiki.svelte'
  let { onaccepted }: { onaccepted?: () => void } = $props()
  import { reviewDiff } from '@/lib/reviewDiff'
  import { onMount, onDestroy } from 'svelte'
  let disposed = false
  onDestroy(() => { disposed = true })
  let proposals = $state<WikiProposal[]>([])
  let selected = $state<WikiProposal | null>(null)
  let draft = $state('')
  let busy = $state(false)
  let error = $state('')
  let checked = $state(false)
  let cacheConflict = $state(false)
  const dirty = $derived(selected !== null && draft !== selected.content)
  const pending = $derived(proposals.filter(p => p.status === 'pending'))
  const cacheKey = (id: string) => `rag-review-draft:${apiUrl('/')}:${id}`
  function cache() {
    checked = false
    if (cacheConflict) return
    if (!selected) return
    try {
      sessionStorage.setItem(cacheKey(selected.id), JSON.stringify({content:draft, revision:selected.revision}))
      sessionStorage.setItem(cacheKey('selected'), selected.id)
    } catch { error = 'Не удалось сохранить локальный черновик. Сохраните предложение перед уходом со страницы.' }
  }
  function select(p: WikiProposal) {
    selected = p; draft = p.content; checked = false; cacheConflict = false
    try {
      const saved = sessionStorage.getItem(cacheKey(p.id))
      if (p.status === 'pending' && saved) {
        const cached = JSON.parse(saved)
        if (typeof cached.content === 'string') {
          draft = cached.content
          cacheConflict = cached.revision !== p.revision && draft !== p.content
          if (cacheConflict) error = 'Сохранённая версия предложения изменилась. Скопируйте нужные правки из локального текста, отмените локальные изменения и повторно внесите их в свежую версию.'
        }
      }
    }
    catch { /* Server draft remains available. */ }
  }
  function discard() {
    if (!selected) return
    draft = selected.content; checked = false; cacheConflict = false; error = ''
    try { sessionStorage.removeItem(cacheKey(selected.id)) } catch { /* No local cache. */ }
  }
  function replace(p: WikiProposal) { try { sessionStorage.removeItem(cacheKey(p.id)) } catch { /* No cache. */ }; proposals = [p, ...proposals.filter(item => item.id !== p.id)]; select(p) }
  export async function create(document_id: string) {
    if (dirty || busy) { error = 'Сначала сохраните текущее предложение.'; return }
    busy = true; error = ''
    try { const result = await api.changeProposal({action:'create', document_id}); if (!disposed) replace(result) }
    catch (e) { error = String(e) }
    finally { busy = false }
  }
  async function load() {
    if (dirty || busy) return
    busy = true; error = ''
    try {
      const result = await api.proposals(); if (disposed) return; proposals = result.items; selected = null; checked = false
      try { const previous = proposals.find(p => p.id === sessionStorage.getItem(cacheKey('selected')) && p.status === 'pending'); if (previous) select(previous) } catch { /* Optional local recovery. */ }
    }
    catch (e) { error = String(e).includes('HTTP 404:') ? 'Для предложений требуется обновление сервера.' : String(e) }
    finally { busy = false }
  }
  async function change(action: 'save' | 'accept' | 'reject') {
    if (!selected || busy || cacheConflict) return
    const p = selected
    busy = true; error = ''
    try {
      const result = await api.changeProposal(action === 'save'
        ? {action, id:p.id, revision:p.revision, content:draft}
        : {action, id:p.id, revision:p.revision})
      if (disposed) return
      replace(result)
      if (action === 'accept') { void wiki.loadCatalog(); onaccepted?.() }
    } catch (e) { error = String(e).includes('HTTP 409:') ? 'Статья, источник или предложение изменились. Ваш текст сохранён в этом окне. Создайте новое предложение на свежих источниках и перенесите проверенные изменения.' : String(e) }
    finally { busy = false }
  }
  onMount(() => { void load() })
</script>
<section aria-label="Предложения изменений" class="proposals">
  <header><h2>Предложения изменений</h2><button disabled={busy || dirty} onclick={load}>Обновить список</button></header>
  {#if error}<p role="alert">{error}</p>{/if}
  <p>{pending.length} ожидают проверки. Сохранённые предложения доступны после перезапуска.</p>
  {#each pending as p (p.id)}<button disabled={busy || dirty} onclick={() => select(p)}>{p.base.title} · черновик {p.revision}</button>{/each}
  <details><summary>История принятых и отклонённых</summary>{#each proposals.filter(p => p.status !== 'pending') as p (p.id)}<button disabled={busy || dirty} onclick={() => select(p)}>{p.base.title} · {p.status === 'accepted' ? 'Принято' : 'Отклонено'}</button>{/each}</details>
  {#if selected}
    <h3>{selected.base.title} · {selected.status === 'accepted' ? 'Принято' : selected.status === 'rejected' ? 'Отклонено' : 'На проверке'}</h3>
    <p>Основание: версия статьи {selected.base.revision}. Сравните исходный текст слева и предложение справа.</p>
    <div class="comparison">
      <label>Исходная статья<textarea readonly value={selected.base.content} rows="16"></textarea></label>
      <label>Предложение<textarea aria-label="Текст предложения" bind:value={draft} oninput={cache} disabled={busy || selected.status !== 'pending'} rows="16"></textarea></label>
    </div>
    <details><summary>Показать изменения строк</summary><div class="diff">{#each reviewDiff(selected.base.content, draft) as line}<pre class:added={line.kind === 'added'} class:removed={line.kind === 'removed'}>{line.kind === 'added' ? '+ ' : line.kind === 'removed' ? '− ' : '  '}{line.text}</pre>{/each}</div></details>
    <details><summary>Источники на момент подготовки ({selected.sources.length})</summary>
      {#each selected.sources as source (source.document_id)}<details><summary>{source.uri || source.document_id}{source.content === null ? ' — отсутствует' : ''}</summary><pre>{source.content ?? 'Источник отсутствовал. Удалите или исправьте неподтверждённые утверждения перед принятием.'}</pre></details>{/each}
      {#if !selected.sources.length}<p>Связи с источниками не найдены. Это предложение не подтверждает достоверность статьи.</p>{/if}
    </details>
    {#if selected.status === 'pending'}
      <div class="actions"><button disabled={busy || !dirty || cacheConflict} onclick={() => change('save')}>Сохранить предложение</button>
        <button disabled={busy || dirty} onclick={() => change('reject')}>Отклонить</button></div>
      <label class="confirm"><input type="checkbox" bind:checked disabled={busy || dirty} /> Я проверил текст и источники, включая отсутствующие; предложение можно опубликовать.</label>
      <button disabled={busy || dirty || !checked || cacheConflict} onclick={() => change('accept')}>Принять и обновить статью</button>
      {#if dirty}<p>Есть несохранённые изменения. Сохраните предложение перед принятием.</p><button disabled={busy} onclick={discard}>Отменить локальные изменения</button>{/if}
    {/if}
  {/if}
</section>
<style>
  .proposals { border: 1px solid var(--border); border-radius: 12px; padding: 20px; margin-bottom: 24px; }
  header, .actions { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:12px; }
  h2 { margin:0; } h3 { margin-top:24px; }
  p, label { line-height:1.6; }
  .comparison { display:grid; grid-template-columns:1fr 1fr; gap:16px; }
  textarea { display:block; box-sizing:border-box; width:100%; padding:12px; background:var(--bg-elevated); color:var(--text); border:1px solid var(--border); border-radius:6px; resize:vertical; }
  button { cursor:pointer; margin:4px 0; padding:8px 12px; background:var(--bg-elevated); color:var(--text); border:1px solid var(--border); border-radius:6px; }
  button:disabled { opacity:.5; cursor:default; }
  pre { white-space:pre-wrap; overflow-wrap:anywhere; max-height:350px; overflow:auto; }
  details { margin:16px 0; } summary { cursor:pointer; overflow-wrap:anywhere; }
  .diff pre { margin:0; padding:2px 8px; }
  .added { background:#14532d44; }
  .removed { background:#7f1d1d44; }
  .confirm { display:block; margin:16px 0; }
  @media(max-width:850px) { .comparison { grid-template-columns:1fr; } }
</style>
