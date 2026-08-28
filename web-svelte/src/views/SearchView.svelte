<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { goWiki } from '@/lib/router.svelte'
  import { api } from '@/api/client'

  interface Hit {
    kind: string
    title: string
    id: string
    detail?: string
  }

  let q = $state('')
  let busy = $state(false)
  let err = $state<string | null>(null)
  let hits = $state<Hit[]>([])

  async function run() {
    const query = q.trim()
    if (!query) return
    busy = true
    err = null
    hits = []
    try {
      if (!wiki.pages.length) await wiki.loadCatalog()
      const ql = query.toLowerCase()
      for (const p of wiki.pages) {
        if (`${p.title} ${p.slug} ${p.summary ?? ''}`.toLowerCase().includes(ql)) {
          hits.push({
            kind: 'wiki',
            title: p.title || p.slug,
            id: p.id,
            detail: p.summary || p.slug,
          })
        }
      }
      try {
        const node = (await api.findNode(query)) as {
          id?: string
          label?: string
          kind?: string
          document_id?: string | null
        }
        if (node?.id) {
          hits.unshift({
            kind: node.kind || 'node',
            title: node.label || node.id,
            id: node.document_id || node.id,
            detail: node.id,
          })
        }
      } catch {
        /* no graph hit */
      }
    } catch (e) {
      err = e instanceof Error ? e.message : String(e)
    } finally {
      busy = false
    }
  }

  function open(id: string) {
    goWiki(id)
  }
</script>

<div class="search">
  <h1>{ui.t('searchHeading')}</h1>
  <p class="sub">{ui.t('searchSubtitle')}</p>
  <form
    class="row"
    onsubmit={(e) => {
      e.preventDefault()
      void run()
    }}
  >
    <input bind:value={q} type="search" placeholder={ui.t('searchPlaceholder')} />
    <button type="submit" disabled={busy}>{busy ? '…' : ui.t('search')}</button>
  </form>
  {#if err}
    <p class="err">{err}</p>
  {/if}
  <ul>
    {#each hits as h, i (i)}
      <li>
        <button type="button" class="hit" onclick={() => open(h.id)}>
          <span class="k">{h.kind}</span>
          <span class="col">
            <span class="t">{h.title}</span>
            <span class="d">{h.detail}</span>
          </span>
        </button>
      </li>
    {/each}
    {#if !hits.length && !busy}
      <li class="empty">{ui.t('searchNoResults')}</li>
    {/if}
  </ul>
</div>

<style>
  .search {
    max-width: 720px;
    margin: 0 auto;
    padding: 32px 20px;
    overflow: auto;
    width: 100%;
  }
  h1 {
    margin: 0 0 4px;
    letter-spacing: -0.03em;
  }
  .sub {
    color: var(--text-muted);
    margin: 0 0 20px;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  input {
    flex: 1;
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    border-radius: 10px;
    padding: 12px 14px;
    outline: none;
  }
  input:focus {
    border-color: var(--accent);
  }
  .row button {
    border: none;
    background: var(--accent);
    color: white;
    border-radius: 10px;
    padding: 0 16px;
    cursor: pointer;
    font-weight: 600;
  }
  .row button:disabled {
    opacity: 0.5;
  }
  ul {
    list-style: none;
    margin: 20px 0 0;
    padding: 0;
  }
  li {
    border-radius: 10px;
    border: 1px solid transparent;
  }
  li:hover {
    background: var(--bg-hover);
    border-color: var(--border);
  }
  .hit {
    display: flex;
    gap: 12px;
    width: 100%;
    text-align: left;
    padding: 12px;
    cursor: pointer;
    border: none;
    background: transparent;
    color: inherit;
    border-radius: 10px;
  }
  .k {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-faint);
    min-width: 48px;
    padding-top: 3px;
  }
  .col {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .t {
    font-weight: 600;
  }
  .d {
    font-size: 13px;
    color: var(--text-muted);
  }
  .err {
    color: var(--danger);
  }
  .empty {
    color: var(--text-muted);
    text-align: center;
    padding: 12px;
  }
</style>
