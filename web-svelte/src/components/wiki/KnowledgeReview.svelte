<script lang="ts">
  import ReviewProposals from './ReviewProposals.svelte'
  let proposals: ReviewProposals | undefined = $state()
  import { api, type ReviewEvidence } from '@/api/client'
  import { goWiki } from '@/lib/router.svelte'
  let items = $state<ReviewEvidence[]>([])
  let loading = $state(false)
  let loaded = $state(false)
  let error = $state('')
  let filter = $state('')
  let source = $state('')
  let sourceTitle = $state('')
  let sourceLoading = $state(false)
  let generation = 0
  const groups = $derived.by(() => {
    const result = new Map<string, ReviewEvidence[]>()
    for (const item of items) {
      const group = result.get(item.wiki_id) ?? []
      group.push(item)
      result.set(item.wiki_id, group)
    }
    return [...result.values()].filter(group => group.some(item =>
      `${item.wiki_title} ${item.raw_title} ${item.raw_uri}`.toLowerCase().includes(filter.toLowerCase())))
  })
  async function load() {
    loading = true
    error = ''
    try { items = (await api.wikiReview()).items; loaded = true }
    catch (e) { error = String(e).includes('HTTP 404:') ? 'Подключённый сервер пока не поддерживает очередь проверки. Требуется обновить сервер.' : String(e) }
    finally { loading = false }
  }
  async function preview(item: ReviewEvidence) {
    const current = ++generation
    sourceTitle = item.raw_title || item.raw_uri
    source = ''
    sourceLoading = true
    try {
      const doc = await api.document({ id: item.raw_id })
      if (current === generation) source = doc.content
    } catch (e) { if (current === generation) source = `Не удалось открыть источник: ${String(e)}` }
    finally { if (current === generation) sourceLoading = false }
  }
  function task(group: ReviewEvidence[]) {
    return `Проверь актуальность статьи ${group[0].wiki_uri} (id: ${group[0].wiki_id}).\nИсточники:\n${group.map(item => `- ${item.raw_uri || item.raw_id}: ${item.link_kind === 'source_missing' ? 'источник отсутствует' : 'требуется проверка'}`).join('\n')}\nСначала создай предложение через POST /v1/wiki-proposals (action=create, document_id статьи). Используй base и sources из полученного снимка для подготовки текста. Сохрани его через action=save с id и revision предложения. Покажи изменения и подтверждения человеку; action=accept вызывай после принятия предложения. Не выдумывай содержимое отсутствующих источников.`
  }
</script>

<section class="review" aria-label="Проверка знаний">
  <header><div><h2>Проверка знаний</h2><p>Какие статьи требуют проверки после изменения источников.</p></div>
    <button disabled={loading} onclick={load}>{loading ? 'Проверяем…' : loaded ? 'Проверить снова' : 'Проверить актуальность'}</button></header>
  {#if error}<p role="alert">Проверка не выполнена. {error}</p>{/if}
  {#if loaded && !error}
    <p>{new Set(items.map(item => item.wiki_id)).size} статей требуют проверки · {items.length} связей с источниками</p>
    {#if items.length}
      <input aria-label="Фильтр очереди проверки" placeholder="Найти статью или источник" bind:value={filter} />
      {#each groups as group (group[0].wiki_id)}
        <article>
          <h3><button onclick={() => goWiki(group[0].wiki_id)}>{group[0].wiki_title || group[0].wiki_uri}</button></h3>
          <ul>{#each group as item (item.raw_id)}<li>
            <span>{item.link_kind === 'source_missing' ? 'Источник отсутствует' : item.link_kind === 'source_version' ? 'Содержимое изменилось' : 'Источник новее статьи — нужна проверка'}:</span>
            {#if item.link_kind === 'source_missing'}<code>{item.raw_uri || item.raw_id}</code>
            {:else}<button onclick={() => preview(item)}>{item.raw_title || item.raw_uri}</button>{/if}
          </li>{/each}</ul>
          <button onclick={() => proposals?.create(group[0].wiki_id)}>Подготовить предложение</button>
          <details><summary>Задание для агента</summary><textarea aria-label="Задание для агента" readonly value={task(group)} rows="8"></textarea></details>
        </article>
      {/each}
      {#if !groups.length}<p>По этому фильтру ничего не найдено.</p>{/if}
    {:else}<p>Изменений среди отслеживаемых источников не обнаружено. Статьи без связей с источниками этой проверкой не подтверждаются.</p>{/if}
  {/if}
  {#if sourceTitle}<aside><header><h3>{sourceTitle}</h3><button onclick={() => { generation++; sourceTitle = ''; source = '' }}>Закрыть источник</button></header><pre>{sourceLoading ? 'Загрузка…' : source}</pre></aside>{/if}
</section>

<ReviewProposals bind:this={proposals} onaccepted={() => { void load() }} />

<style>
  .review { border: 1px solid var(--border); border-radius: 12px; padding: 20px; margin-bottom: 24px; }
  header { display: flex; justify-content: space-between; align-items: start; gap: 16px; flex-wrap: wrap; }
  h2, h3 { margin: 0 0 8px; }
  p, li { line-height: 1.6; }
  article { border-top: 1px solid var(--border); padding: 16px 0; }
  button { cursor: pointer; padding: 7px 10px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-elevated); color: var(--text); }
  button:disabled { opacity: .6; }
  input, textarea { width: 100%; box-sizing: border-box; background: var(--bg-elevated); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 10px; }
  textarea { margin-top: 10px; }
  li { overflow-wrap: anywhere; margin-bottom: 8px; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 420px; overflow: auto; }
  aside { border-top: 1px solid var(--border); padding-top: 16px; }
  summary { cursor: pointer; }
</style>
