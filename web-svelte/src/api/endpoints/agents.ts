import { http, type RequestOptions } from '../http'
import type {
  ActivityItem,
  AgentInfo,
  CallRow,
  ItemsResponse,
  KgFact,
  KgStats,
  OpsLogItem,
  SyncState,
} from '../types'

async function items<T>(path: string, options?: RequestOptions): Promise<T[]> {
  return (await http.get<ItemsResponse<T>>(path, options)).items ?? []
}

/** MCP clients, their activity, the mutation journal, sync topology and KG facts. */
export const agentsApi = {
  agents: (options?: RequestOptions) => items<AgentInfo>('/v1/agents', options),
  /** Recent MCP tool calls (in-memory read stream). */
  calls: (limit: number, options?: RequestOptions) => items<CallRow>(`/v1/calls?limit=${limit}`, options),
  /** Append-only mutation journal, newest first. */
  opsLog: (limit: number, options?: RequestOptions) =>
    items<OpsLogItem>(`/v1/ops-log?limit=${limit}`, options),
  activity: (limit: number, options?: RequestOptions) =>
    items<ActivityItem>(`/v1/activity?limit=${limit}`, options),
  syncStatus: async (options?: RequestOptions): Promise<SyncState> =>
    (await http.get<{ sync: SyncState }>('/v1/sync/status', options)).sync,

  kgStats: async (options?: RequestOptions): Promise<KgStats> =>
    (await http.get<{ stats?: KgStats }>('/v1/kg/stats', options)).stats ?? {},
  /** KG facts; gateways differ on the envelope (`items`/`facts`, `count`/`total`). */
  kgFacts: async (limit: number, options?: RequestOptions): Promise<{ items: KgFact[]; total: number }> => {
    const response = await http.get<{ items?: KgFact[]; facts?: KgFact[]; count?: number; total?: number }>(
      `/v1/kg?limit=${limit}`,
      options,
    )
    const facts = response.items ?? response.facts ?? []
    return { items: facts, total: response.count ?? response.total ?? facts.length }
  },
}
