<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
</script>

<div class="editor">
  <input
    class="title"
    bind:value={wiki.draftTitle}
    placeholder={ui.t('untitled')}
    oninput={() => wiki.markDirty()}
  />
  <textarea
    class="body"
    bind:value={wiki.draftContent}
    placeholder={ui.t('writeMarkdown')}
    spellcheck="true"
    oninput={() => wiki.markDirty()}
  ></textarea>
  <p class="hint">{ui.t('casHint')}</p>
</div>

<style>
  .editor {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: 20px 48px 40px;
    max-width: 860px;
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
