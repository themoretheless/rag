import { errorMessage } from '@/lib/async'
import { api } from './client'

/**
 * A gateway read that several screens (and the shell poller) want at the same time.
 *
 * - concurrent `refresh()` calls share one request;
 * - `fresh(maxAgeMs)` lets a poller reuse a snapshot another screen has just fetched;
 * - the last good `value` stays available (and reactive) after a failed refresh.
 */
export class SharedResource<T> {
  /** Last successful snapshot; replaced wholesale, never mutated. */
  value = $state.raw<T | null>(null)
  /** Message of the last failed refresh, cleared by the next success. */
  error = $state<string | null>(null)

  private inflight: Promise<T> | null = null
  private fetchedAt = Number.NEGATIVE_INFINITY

  constructor(private readonly load: () => Promise<T>) {}

  /** Fetch now. Rejects with the loader's error; `value` keeps the previous snapshot. */
  refresh(): Promise<T> {
    this.inflight ??= this.load()
      .then(
        (value) => {
          this.value = value
          this.error = null
          this.fetchedAt = Date.now()
          return value
        },
        (cause: unknown) => {
          this.error = errorMessage(cause)
          throw cause
        },
      )
      .finally(() => {
        this.inflight = null
      })
    return this.inflight
  }

  /** The current snapshot when it is at most `maxAgeMs` old, otherwise a refresh. */
  fresh(maxAgeMs: number): Promise<T> {
    const value = this.value
    if (value !== null && Date.now() - this.fetchedAt <= maxAgeMs) return Promise.resolve(value)
    return this.refresh()
  }
}

/** `GET /v1/status` — console, models and evaluation screens. */
export const statusResource = new SharedResource(() => api.status())

/** `GET /v1/agents` — shell avatars, console, agents and sync screens. */
export const agentsResource = new SharedResource(() => api.agents())
