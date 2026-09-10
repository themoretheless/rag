<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { api } from '@/api/client'
  import type { WikiPageMeta } from '@/api/types'
  import { linkAtCaret, insertWikiLink, wikiTemplates } from '@/lib/wikiEditing'
  import { tick, onDestroy } from 'svelte'
  let body: HTMLTextAreaElement
  let matches = $state<WikiPageMeta[]>([])
  let completion = $state<ReturnType<typeof linkAtCaret>>(null)
  let previewTitle = $state('')
  let preview = $state('')
  let searchError = $state('')
  const references = $derived((wiki.current?.source_versions ?? []).filter(ref => ref && (typeof ref.document_id === 'string' || typeof ref.uri === 'string')))
  let request = 0
  let previewRequest = 0
  let timer: ReturnType<typeof setTimeout> | undefined
  onDestroy(() => { clearTimeout(timer); request++; previewRequest++ })
  function complete() {
    clearTimeout(timer)
    const version = ++request
    completion = linkAtCaret(wiki.draftContent, body.selectionStart)
    matches = []; searchError = ''
    if (!completion) return
    const query = completion.query
    timer = setTimeout(async () => {
      try {
        const response = await api.wikiList({ q:query, limit:8 })
        if (version === request) matches = response.pages
      } catch { if (version === request) searchError = 'Не удалось загрузить подсказки.' }
    }, 180)
  }
  async function insert(page: WikiPageMeta) {
    completion = linkAtCaret(wiki.draftContent, body.selectionStart)
    if (!completion) return
    const result = insertWikiLink(wiki.draftContent,completion.start,completion.end,page.slug,page.title)
    wiki.draftContent = result.text; wiki.markDirty(); completion = null; matches = []; request++
    await tick(); body.focus(); body.setSelectionRange(result.caret,result.caret)
  }
  async function show(page: { id?: string; title: string; uri?: string }) {
    const version = ++previewRequest
    previewTitle = page.title; preview = 'Загрузка…'
    try { const doc = await api.document({id:page.id, uri:page.uri}); if (version === previewRequest) preview = doc.content }
    catch (e) { if (version === previewRequest) preview = `Не удалось открыть страницу: ${String(e)}` }
  }
  function template(key: keyof typeof wikiTemplates) {
    completion = null; matches = []; request++
    wiki.draftContent += (wiki.draftContent ? '\n\n' : '') + wikiTemplates[key]
    wiki.markDirty()
  }
</script>

<div class="editor">
  <input
    class="title"
    bind:value={wiki.draftTitle}
    placeholder={ui.t('untitled')}
    oninput={() => wiki.markDirty()}
  />
  <details class="templates"><summary>Добавить шаблон</summary><div>
    <button onclick={() => template('decision')}>Решение</button><button onclick={() => template('guide')}>Инструкция</button><button onclick={() => template('service')}>Обзор сервиса</button><button onclick={() => template('research')}>Исследование</button>
  </div><p>Шаблон добавляется в конец текста.</p></details>
  {#if references.length}<details><summary>Источники статьи</summary>{#each references as ref}<button onclick={() => show({ id:ref.document_id, uri:ref.uri, title:ref.uri || ref.document_id || 'Источник' })}>{ref.uri || ref.document_id}</button>{/each}</details>{/if}
  <div class="workspace">
  <div class="writing">
  <textarea
    bind:this={body}
    aria-label="Текст статьи"
    class="body"
    bind:value={wiki.draftContent}
    placeholder={ui.t('writeMarkdown')}
    spellcheck="true"
    oninput={() => { wiki.markDirty(); complete() }}
    onclick={complete}
    onkeyup={(event) => { if (event.key.startsWith('Arrow') || ['Home','End'].includes(event.key)) complete() }}
  ></textarea>
  {#if completion}<div class="completions"><p>Выберите страницу для ссылки</p>{#each matches as page (page.id)}<div><button onclick={() => insert(page)}>{page.title || page.slug}</button><button onclick={() => show(page)}>Просмотр</button></div>{/each}{#if searchError}<p role="alert">{searchError}</p>{/if}</div>{/if}
  </div>
  {#if previewTitle}<aside><header><strong>{previewTitle}</strong><button onclick={() => { previewRequest++; previewTitle = '' }}>Закрыть</button></header><pre>{preview}</pre></aside>{/if}
  </div>
  <p class="hint">{ui.t('casHint')}</p>
</div>

<style>
  .workspace { display:flex; flex:1; min-height:0; gap:16px; }
  .writing { flex:1; min-width:0; display:flex; flex-direction:column; }
  aside { flex:1; min-width:0; border:1px solid var(--border); border-radius:8px; padding:12px; overflow:auto; }
  aside pre { white-space:pre-wrap; overflow-wrap:anywhere; }
  header, .completions > div { display:flex; justify-content:space-between; gap:8px; }
  button { background:var(--bg-elevated); color:var(--text); border:1px solid var(--border); padding:6px 10px; border-radius:6px; cursor:pointer; }
  .templates summary { cursor:pointer; }
  .templates p, .completions p { font-size:12px; color:var(--text-muted); }
  .completions { max-height:240px; overflow:auto; padding:8px; border:1px solid var(--border); }
  @media(max-width:850px) { .workspace { flex-direction:column; } }

  .editor {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: 20px 48px 40px;
    max-width: 1400px;
    margin: 0 auto;
    width: 100%;
    gap: 12px;
  }
  .title {
    border: none;
    background: transparent;
    font-size: 32px;
    font-weight: 700;
    letter-spacing: -0.03em;
    outline: none;
    padding: 0;
  }
  .body {
    flex: 1;
    min-height: 280px;
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    border-radius: 12px;
    padding: 16px;
    resize: none;
    outline: none;
    line-height: 1.55;
    font-family: var(--mono);
    font-size: 13.5px;
  }
  .body:focus {
    border-color: var(--accent);
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-faint);
  }
</style>
