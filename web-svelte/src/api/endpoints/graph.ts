import { http, type RequestOptions } from '../http'
import type { GraphEdge, GraphView, ItemsResponse } from '../types'

/** Knowledge graph views. */
export const graphApi = {
  graph: (
    opts?: { max_nodes?: number; include_tags?: boolean; project?: string },
    options?: RequestOptions,
  ) => {
    const q = new URLSearchParams()
    if (opts?.max_nodes) q.set('max_nodes', String(opts.max_nodes))
    if (opts?.include_tags) q.set('include_tags', 'true')
    if (opts?.project) q.set('project', opts.project)
    const s = q.toString()
    return http.get<GraphView>(`/v1/graph${s ? `?${s}` : ''}`, options)
  },

  neighbors: (seed: string, depth = 1, max_nodes = 100, include_tags = false, project = '') => {
    const q = new URLSearchParams({
      seed,
      depth: String(depth),
      max_nodes: String(max_nodes),
    })
    if (include_tags) q.set('include_tags', 'true')
    if (project) q.set('project', project)
    return http.get<GraphView>(`/v1/neighbors?${q.toString()}`)
  },

  /** Cross-project "tunnel" edges, overlaid on whichever view is visible. */
  tunnels: async (options?: RequestOptions): Promise<GraphEdge[]> =>
    (await http.get<ItemsResponse<GraphEdge>>('/v1/tunnels', options)).items ?? [],
}
