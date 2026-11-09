import { api } from '@/api/client'
import type { CallRow } from '@/api/types'
import { createRequestGate, errorMessage, withTimeout } from '@/lib/async'
import { usePoll } from '@/lib/poll'

/** `empty` is a healthy stream without calls; `stale` keeps the last good snapshot. */
export type CallStreamState = 'loading' | 'live' | 'empty' | 'stale' | 'error'

export const CALL_LIMIT = 100
export const CALL_TIMEOUT_MS = 8_000
export const CALL_POLL_MS = 5_000

/**
 * Snapshot of the recent MCP calls, refreshed by polling. A failed refresh keeps
 * the previous snapshot and only flips the state to `stale` (or `error` when there
 * has never been a snapshot).
 */
export class CallStream {
  /** Replaced wholesale on every successful refresh. */
  calls = $state.raw<CallRow[]>([])
  state = $state<CallStreamState>('loading')
  error = $state('')
  /** Time of the last successful refresh (ms since epoch). */
  updatedAt = $state<number | null>(null)

  private loaded = false
  private polling = false
  private readonly gate = createRequestGate()

  /** Refresh once. Rejects on failure so a caller can report the panel. */
  async fetch(): Promise<void> {
    const ticket = this.gate.next()
    if (!this.loaded) this.state = 'loading'
    try {
      const calls = await withTimeout(api.calls(CALL_LIMIT, { signal: ticket.signal }), CALL_TIMEOUT_MS)
      if (!ticket.current) return
      this.calls = calls
      this.loaded = true
      this.error = ''
      this.updatedAt = Date.now()
      this.state = calls.length ? 'live' : 'empty'
    } catch (cause) {
      if (!ticket.current) return
      this.error = errorMessage(cause)
      this.state = this.loaded ? 'stale' : 'error'
      throw cause
    }
  }

  /** Poll tick: skipped while the previous tick is still running; never rejects. */
  async poll(): Promise<void> {
    if (this.polling) return
    this.polling = true
    try {
      await this.fetch()
    } catch {
      // fetch() retains the last valid snapshot and exposes stale/error itself.
    } finally {
      this.polling = false
    }
  }
}

/** A call stream that polls for as long as the calling component is mounted. */
export function useCallStream(intervalMs: number = CALL_POLL_MS): CallStream {
  const stream = new CallStream()
  usePoll(() => void stream.poll(), intervalMs)
  return stream
}
