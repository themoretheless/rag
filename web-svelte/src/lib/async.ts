/** Small async building blocks shared by stores and views. */

/** Human-readable text of anything that can be thrown; `fallback` replaces non-Error values. */
export function errorMessage(cause: unknown, fallback?: string): string {
  return cause instanceof Error ? cause.message : (fallback ?? String(cause))
}

/**
 * Run independent dashboard loaders without letting one failed panel hide the rest.
 * Returns `label: message` for each failed panel.
 */
export async function loadPanels(
  panels: readonly (readonly [label: string, load: () => Promise<void>])[],
): Promise<string[]> {
  const outcomes = await Promise.all(
    panels.map(async ([label, load]) => {
      try {
        await load()
        return null
      } catch (err) {
        const message = errorMessage(err)
        return message ? `${label}: ${message}` : label
      }
    }),
  )
  return outcomes.filter((label): label is string => label !== null)
}

export interface Debounced<Args extends unknown[]> {
  (...args: Args): void
  /** Drop the pending call, if any. */
  cancel(): void
  /** True while a call is waiting for its delay to elapse. */
  readonly pending: boolean
}

/** Trailing-edge debounce: only the last call within `ms` runs. */
export function debounce<Args extends unknown[]>(fn: (...args: Args) => void, ms: number): Debounced<Args> {
  let timer: ReturnType<typeof setTimeout> | null = null
  const cancel = () => {
    if (timer === null) return
    clearTimeout(timer)
    timer = null
  }
  const debounced = (...args: Args) => {
    cancel()
    timer = setTimeout(() => {
      timer = null
      fn(...args)
    }, ms)
  }
  return Object.defineProperties(debounced, {
    cancel: { value: cancel },
    pending: { get: () => timer !== null },
  }) as Debounced<Args>
}

/** One generation of a gated request. */
export interface RequestTicket {
  /** Aborted as soon as a newer request starts or the gate is cancelled. */
  readonly signal: AbortSignal
  /** False once superseded: the caller must drop its result (and its error). */
  readonly current: boolean
}

export interface RequestGate {
  /** Supersede whatever is in flight and open a new generation. */
  next(): RequestTicket
  /** Ticket of the newest generation, without superseding it. */
  readonly latest: RequestTicket
  /** Invalidate the in-flight generation without starting a request. */
  cancel(): void
}

/**
 * "Latest request wins" guard. Each `next()` aborts the previous generation, so a
 * slow stale response can neither overwrite newer state nor keep the network busy.
 *
 *   const ticket = gate.next()
 *   const data = await load({ signal: ticket.signal })
 *   if (!ticket.current) return
 */
export function createRequestGate(): RequestGate {
  const open = (): RequestTicket => {
    const controller = new AbortController()
    const ticket: RequestTicket = {
      signal: controller.signal,
      get current() {
        return latest === ticket
      },
    }
    controllers.set(ticket, controller)
    return ticket
  }
  const controllers = new WeakMap<RequestTicket, AbortController>()
  let latest = open()
  const supersede = () => {
    controllers.get(latest)?.abort()
    latest = open()
    return latest
  }
  return {
    next: supersede,
    get latest() {
      return latest
    },
    cancel: () => {
      supersede()
    },
  }
}

/**
 * Load something once and share it: concurrent and later callers get the same promise.
 * A rejection is not kept, so the call after a failure tries again instead of
 * replaying the old error forever.
 */
export function lazyPromise<T>(load: () => Promise<T>): () => Promise<T> {
  let pending: Promise<T> | null = null
  return () => {
    pending ??= load().catch((cause: unknown) => {
      pending = null
      throw cause
    })
    return pending
  }
}

/** Reject with `Timeout after N ms` when `promise` has not settled within `timeoutMs`. */
export async function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => {
          reject(new Error(`Timeout after ${timeoutMs} ms`))
        }, timeoutMs)
      }),
    ])
  } finally {
    clearTimeout(timer)
  }
}
