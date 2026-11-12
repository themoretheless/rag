import { api } from '@/api/client'
import { agentsResource } from '@/api/resources.svelte'
import type { AgentInfo, HealthResponse } from '@/api/types'
import { createRequestGate, errorMessage } from '@/lib/async'
import { startPoll } from '@/lib/poll'

/** Interval of the shell's background health check. */
export const HEALTH_POLL_MS = 15_000
/** The poller reuses an agents snapshot another screen fetched this recently. */
export const AGENTS_REUSE_MS = 10_000

/** Gateway health badge and online-agent avatars of the application shell. */
export class ShellHealth {
  health = $state<HealthResponse | null>(null)
  healthError = $state<string | null>(null)
  agents = $state<AgentInfo[]>([])
  agentsError = $state<string | null>(null)

  private readonly gate = createRequestGate()

  /**
   * Refresh both snapshots. `agentsMaxAgeMs` lets the background poller reuse an
   * agents snapshot a screen has just fetched.
   */
  async check(agentsMaxAgeMs = 0) {
    const ticket = this.gate.next()

    // Keep the two shell snapshots independent: a broken optional agents panel
    // must not turn a healthy gateway red (and vice versa).
    const healthTask = api.health().then(
      (health) => {
        if (!ticket.current) return
        this.health = health
        this.healthError = null
      },
      (cause: unknown) => {
        if (!ticket.current) return
        this.healthError = errorMessage(cause)
        this.health = null
      },
    )

    const agentsTask = (
      agentsMaxAgeMs > 0 ? agentsResource.fresh(agentsMaxAgeMs) : agentsResource.refresh()
    ).then(
      (agents) => {
        if (!ticket.current) return
        this.agents = agents
        this.agentsError = null
      },
      (cause: unknown) => {
        if (!ticket.current) return
        // Preserve the last valid snapshot for diagnostics. TopBar hides it
        // while stale, so old `online` flags are never presented as current.
        this.agentsError = errorMessage(cause)
      },
    )

    await Promise.all([healthTask, agentsTask])
  }

  /**
   * Check now and keep checking in the background (paused while the tab is hidden)
   * so offline recovery shows up. Returns the stop function.
   */
  start(): () => void {
    void this.check()
    return startPoll(() => void this.check(AGENTS_REUSE_MS), HEALTH_POLL_MS)
  }
}
