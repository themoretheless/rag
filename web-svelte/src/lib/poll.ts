import { onMount } from 'svelte'

/**
 * Call `fn` every `ms` while the tab is visible. Ticks that fall into a hidden tab
 * are skipped; if any were skipped, `fn` runs once as soon as the tab is shown again
 * so the screen does not stay stale until the next interval. Returns the stop function.
 *
 * The first call is the caller's job (screens differ in what their initial load does).
 */
export function startPoll(fn: () => void, ms: number): () => void {
  let missed = false
  const hidden = () => document.visibilityState === 'hidden'
  const tick = () => {
    if (hidden()) missed = true
    else fn()
  }
  const onVisibility = () => {
    if (hidden() || !missed) return
    missed = false
    fn()
  }
  const timer = setInterval(tick, ms)
  document.addEventListener('visibilitychange', onVisibility)
  return () => {
    clearInterval(timer)
    document.removeEventListener('visibilitychange', onVisibility)
  }
}

/** Component-scoped `startPoll`: starts on mount, stops on destroy. */
export function usePoll(fn: () => void, ms: number): void {
  onMount(() => startPoll(fn, ms))
}
