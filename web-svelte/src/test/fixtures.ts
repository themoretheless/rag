import type { DocumentBody, WikiPageMeta } from '@/api/types'

/** Shared factories for component tests. */
export function meta(id: string, title: string, extra: Partial<WikiPageMeta> = {}): WikiPageMeta {
  return {
    id,
    uri: `wiki://${id}`,
    slug: id,
    title,
    kind: 'wiki',
    revision: 1,
    etag: 'W/"1"',
    updated_at: '',
    ...extra,
  }
}

export function doc(id: string, content: string, extra: Partial<DocumentBody> = {}): DocumentBody {
  return {
    id,
    uri: `wiki://${id}`,
    title: `Title ${id}`,
    content,
    layer: 'wiki',
    kind: 'wiki',
    revision: 3,
    etag: 'W/"3"',
    ...extra,
  }
}
