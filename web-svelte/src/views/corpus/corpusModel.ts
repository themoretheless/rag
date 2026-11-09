/** Pure logic of the corpus screen. */
import { api } from '@/api/client'
import type { BacklinkItem, DocumentDetail, DocumentRow, DocumentsQuery } from '@/api/types'

/** Catalog page size requested from the gateway. */
export const CATALOG_PAGE_SIZE = 200
/** Chunks fetched with a document for the inspector's chunk map. */
export const DETAIL_CHUNK_LIMIT = 80

/** Catalog filters as the screen holds them. */
export interface CorpusFilters {
  project: string
  room: string
  layer: string
  status: string
  query: string
}

/** `GET /v1/documents` query for the filters; archived rows are requested only when asked for. */
export function documentFilters(filters: CorpusFilters): DocumentsQuery {
  return {
    limit: CATALOG_PAGE_SIZE,
    wing: filters.project,
    room: filters.room,
    layer: filters.layer,
    status: filters.status,
    include_archived: filters.status === 'archived',
    q: filters.query.trim(),
  }
}

/**
 * Append a cursor page to the loaded rows. A row that appears again (the catalog moved
 * between pages) keeps its original position and takes the newer data.
 */
export function mergeRows(existing: readonly DocumentRow[], incoming: readonly DocumentRow[]): DocumentRow[] {
  return [...new Map([...existing, ...incoming].map((row) => [row.id, row])).values()]
}

/** Put a document opened by deep link at the head of the list, without duplicating it. */
export function prependRow(rows: readonly DocumentRow[], row: DocumentRow): DocumentRow[] {
  return [row, ...rows.filter((candidate) => candidate.id !== row.id)]
}

/** UTF-8 size of a document body, e.g. `12 345 байт`. */
export function contentSize(value?: string | null): string {
  if (!value) return '—'
  const bytes = new TextEncoder().encode(value).length
  return `${new Intl.NumberFormat('ru-RU').format(bytes)} байт`
}

export interface DocumentDetailBundle {
  /** Null when the gateway no longer has the document. */
  detail: DocumentDetail | null
  backlinks: BacklinkItem[]
}

/**
 * Everything the inspector shows for one document: the document with its first
 * chunks, plus backlinks. Backlinks are optional — their failure yields an empty list
 * instead of failing the inspector.
 */
export async function loadDocumentDetail(documentId: string): Promise<DocumentDetailBundle> {
  const [details, backlinks] = await Promise.all([
    api.multiGet([documentId], { include_chunks: true, chunk_limit: DETAIL_CHUNK_LIMIT }),
    api.backlinks(documentId).catch(() => null),
  ])
  return { detail: details[0] ?? null, backlinks: backlinks?.backlinks ?? [] }
}
