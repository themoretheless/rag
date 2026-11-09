import { http, type RequestOptions } from '../http'
import type { PackContextRequest, PackContextResponse, SearchRequest, SearchResponse } from '../types'

/** Retrieval and context packing. */
export const searchApi = {
  search: (body: SearchRequest, options?: RequestOptions) =>
    http.post<SearchResponse>('/v1/search', body, options),
  packContext: (body: PackContextRequest, options?: RequestOptions) =>
    http.post<PackContextResponse>('/v1/pack-context', body, options),
}
