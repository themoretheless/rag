<script lang="ts">
  import { ui, type Toast } from '@/lib/state/ui.svelte'

  function kindClass(t: Toast): string {
    return t.kind
  }
</script>

<div class="toasts" aria-live="polite">
  {#each ui.toasts as toast (toast.id)}
    <div class="toast {kindClass(toast)}">{toast.text}</div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
    max-width: min(420px, 92vw);
  }
  .toast {
    padding: 10px 14px;
    border-radius: 10px;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    box-shadow: var(--shadow);
    font-size: 13px;
    animation: toast-in 0.16s ease;
  }
  @keyframes toast-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .toast.ok {
    border-color: color-mix(in srgb, var(--ok) 40%, var(--border));
  }
  .toast.error {
    border-color: color-mix(in srgb, var(--danger) 50%, var(--border));
    color: var(--danger);
  }
</style>
