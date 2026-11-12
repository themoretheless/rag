import { http, type RequestOptions } from '../http'
import type {
  DocumentBody,
  DocumentDetail,
  DocumentPage,
  DocumentsQuery,
  ItemsResponse,
  ProjectHome,
  ProjectSummary,
  TaxonomySummary,
} from '../types'

/** Serialized `GET /v1/documents` filters; also the identity of a catalog page sequence. */
export function documentsQuery(query: DocumentsQuery): string {
  // Key order is part of the signature callers compare, so build it explicitly.
  const params = new URLSearchParams()
  if (query.limit !== undefined) params.set('limit', String(query.limit))
  if (query.wing) params.set('wing', query.wing)
  if (query.room) params.set('room', query.room)
  if (query.layer) params.set('layer', query.layer)
  if (query.status) params.set('status', query.status)
  if (query.include_archived) params.set('include_archived', 'true')
  if (query.q) params.set('q', query.q)
  if (query.cursor) params.set('cursor', query.cursor)
  return params.toString()
}

/** Projects (wings), taxonomy and the raw document catalog. */
export const corpusApi = {
  projects: async (options?: RequestOptions): Promise<ProjectSummary[]> =>
    (await http.get<ItemsResponse<ProjectSummary>>('/v1/projects', options)).items ?? [],
  /** Source roots of one project; null when the gateway has no home for it. */
  projectHome: async (project: string, options?: RequestOptions): Promise<ProjectHome | null> =>
    (
      await http.get<{ project?: ProjectHome }>(
        `/v1/project-home?project=${encodeURIComponent(project)}`,
        options,
      )
    ).project ?? null,
  /** Wing / room counts; undefined when the gateway omits the section. */
  taxonomy: async (options?: RequestOptions): Promise<TaxonomySummary | undefined> =>
    (await http.get<{ taxonomy?: TaxonomySummary }>('/v1/taxonomy', options)).taxonomy,

  /** One cursor page of the document catalog. */
  documents: (query: DocumentsQuery, options?: RequestOptions) =>
    http.get<DocumentPage>(`/v1/documents?${documentsQuery(query)}`, options),
  /** Full documents by id, optionally with their first `chunk_limit` chunks. */
  multiGet: async (
    documentIds: string[],
    chunks: { include_chunks: false } | { include_chunks: true; chunk_limit: number },
    options?: RequestOptions,
  ): Promise<DocumentDetail[]> =>
    (
      await http.post<ItemsResponse<DocumentDetail>>(
        '/v1/multi-get',
        { document_ids: documentIds, ...chunks },
        options,
      )
    ).items ?? [],
  document: (
    opts: { id?: string | undefined; uri?: string | undefined; q?: string | undefined },
    options?: RequestOptions,
  ) => {
    const q = new URLSearchParams()
    if (opts.id) q.set('id', opts.id)
    if (opts.uri) q.set('uri', opts.uri)
    if (opts.q) q.set('q', opts.q)
    return http.get<DocumentBody>(`/v1/document?${q.toString()}`, options)
  },
  /** Original source file of a document. */
  sourceFile: (documentId: string) =>
    http.blob(`/v1/source-file?document_id=${encodeURIComponent(documentId)}`),

  /** Queue a `sync_sources` job for an allowed ingest path; resolves to the job id ('' if unknown). */
  startSync: async (body: {
    path: string
    wing?: string | undefined
    room?: string | undefined
  }): Promise<string> => {
    const response = await http.post<{ id?: string; job?: { id?: string } }>('/v1/jobs/sync', body)
    return response.id ?? response.job?.id ?? ''
  },
}
