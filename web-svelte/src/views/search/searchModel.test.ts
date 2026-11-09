import { describe, expect, it } from 'vitest'
import type { PackContextResponse, SearchHit } from '@/api/types'
import {
  buildSearchContextPage,
  corpusLink,
  defaultSearchForm,
  flowColor,
  flowPath,
  flowY,
  packRequest,
  rankFlow,
  searchRequest,
  snapshotOf,
  timingPercent,
} from './searchModel'

function hit(id: string, extra: Partial<SearchHit> = {}): SearchHit {
  return {
    chunk_id: id,
    document_id: `doc-${id}`,
    document_title: `Title ${id}`,
    document_uri: `file:///${id}`,
    chunk_index: 0,
    content: id,
    score: 1,
    ...extra,
  }
}

describe('snapshotOf', () => {
  it('trims text fields and signs the parameter set', () => {
    const snapshot = snapshotOf({
      ...defaultSearchForm(),
      query: '  rrf  ',
      wing: ' rag ',
      documentId: ' d1 ',
    })
    expect(snapshot.query).toBe('rrf')
    expect(snapshot.wing).toBe('rag')
    expect(snapshot.documentId).toBe('d1')
    expect(snapshot.signature).toBe(
      snapshotOf({ ...defaultSearchForm(), query: 'rrf', wing: 'rag', documentId: 'd1' }).signature,
    )
  })

  it('changes the signature when any parameter changes', () => {
    const base = snapshotOf({ ...defaultSearchForm(), query: 'q' })
    expect(snapshotOf({ ...defaultSearchForm(), query: 'q', topK: 9 }).signature).not.toBe(base.signature)
    expect(snapshotOf({ ...defaultSearchForm(), query: 'q', mode: 'lex' }).signature).not.toBe(base.signature)
  })
})

describe('searchRequest', () => {
  it('maps the default form to the wire names and omits empty filters', () => {
    const request = searchRequest(snapshotOf({ ...defaultSearchForm(), query: 'q' }))
    expect(JSON.parse(JSON.stringify(request))).toEqual({
      query: 'q',
      mode: 'hybrid',
      top_k: 8,
      rrf_k: 60,
      max_chunks_per_document: 2,
      context_expansion: 'neighbors',
      neighbor_chunks: 1,
      max_context_tokens: 2000,
      recency_half_life_days: 30,
      timeout_ms: 5000,
    })
  })

  it('sends filters, a positive min_score and the document scope when set', () => {
    const request = searchRequest(
      snapshotOf({
        ...defaultSearchForm(),
        query: 'q',
        wing: 'rag',
        room: 'src',
        layer: 'raw',
        minScore: 0.2,
        documentId: 'd1',
        neighborChunks: 0,
      }),
    )
    expect(request).toMatchObject({
      wing: 'rag',
      room: 'src',
      layer: 'raw',
      min_score: 0.2,
      document_id: 'd1',
    })
    expect(request.context_expansion).toBeUndefined()
    expect(request.neighbor_chunks).toBe(0)
  })
})

describe('packRequest', () => {
  it('copies the hits and carries the token budget and expansion of the search', () => {
    const hits = [hit('a'), hit('b')]
    const request = packRequest(snapshotOf({ ...defaultSearchForm(), query: 'q', maxTokens: 500 }), hits)
    expect(request).toEqual({ hits, max_tokens: 500, context_expansion: 'neighbors', neighbor_chunks: 1 })
    expect(request.hits).not.toBe(hits)
  })
})

describe('corpusLink', () => {
  it('links to the document and keeps the searched scope', () => {
    const snapshot = snapshotOf({ ...defaultSearchForm(), query: 'q', wing: 'rag', room: 'src' })
    expect(corpusLink(hit('a'), snapshot)).toBe('/corpus?document_id=doc-a&q=Title+a&project=rag&room=src')
    expect(corpusLink(hit('a', { document_title: '' }), null)).toBe(
      '/corpus?document_id=doc-a&q=file%3A%2F%2F%2Fa',
    )
  })
})

describe('rank flow', () => {
  it('orders the source columns by their own ranks and keeps fused indices', () => {
    const hits = [
      hit('a', { rank_lex: 3, rank_vec: 1 }),
      hit('b', { rank_lex: 1 }),
      hit('c', { rank_vec: 2 }),
      hit('d', { rank_lex: 2, rank_vec: 5 }),
    ]
    const flow = rankFlow(hits)
    expect(flow.fused.map((row) => row.fusedIndex)).toEqual([0, 1, 2, 3])
    expect(flow.lex.map((row) => [row.hit.chunk_id, row.fusedIndex])).toEqual([
      ['b', 1],
      ['d', 3],
      ['a', 0],
    ])
    expect(flow.vec.map((row) => row.hit.chunk_id)).toEqual(['a', 'c', 'd'])
    expect(flow.connections.map((c) => [c.hit.chunk_id, c.lexIndex, c.vecIndex])).toEqual([
      ['a', 2, 0],
      ['b', 0, -1],
      ['c', -1, 1],
      ['d', 1, 2],
    ])
  })

  it('shows at most eight fused rows', () => {
    const flow = rankFlow(
      Array.from({ length: 12 }, (_, index) => hit(String(index), { rank_lex: index + 1 })),
    )
    expect(flow.fused).toHaveLength(8)
    expect(flow.lex).toHaveLength(8)
    expect(flow.connections).toHaveLength(8)
  })

  it('has stable geometry and a cycling palette', () => {
    expect(flowY(0)).toBe(40.5)
    expect(flowY(2)).toBe(102.5)
    expect(flowPath(300, 40.5, 360, 71.5)).toBe('M 300 40.5 C 330 40.5, 330 71.5, 360 71.5')
    expect(flowColor(0)).toBe('var(--l1)')
    expect(flowColor(8)).toBe(flowColor(0))
  })
})

describe('timingPercent', () => {
  it('clamps to 0..100 and treats unknown values as zero', () => {
    expect(timingPercent(5, 20)).toBe(25)
    expect(timingPercent(30, 20)).toBe(100)
    expect(timingPercent(undefined, 20)).toBe(0)
    expect(timingPercent(5, 0)).toBe(0)
  })
})

describe('buildSearchContextPage', () => {
  const packed: PackContextResponse = {
    ok: true,
    total_tokens: 120,
    max_tokens: 2000,
    omitted_count: 1,
    context_text: 'PACKED TEXT',
    hits: [hit('a'), hit('b')],
  }

  it('builds a wiki draft that records the query, parameters and the packed text', () => {
    const snapshot = snapshotOf({ ...defaultSearchForm(), query: 'how  does\nrrf work', mode: 'lex' })
    const page = buildSearchContextPage(snapshot, packed, 3, 1_700_000_000_000)
    expect(page.slug).toBe(`search-context-${(1_700_000_000_000).toString(36)}`)
    expect(page.title).toBe('Контекст поиска: how does rrf work')
    expect(page.kind).toBe('wiki')
    expect(page.category).toBe('search-context')
    expect(page.summary).toBe('Черновик из 2 хитов для запроса «how does rrf work»')
    expect(page.content.split('\n')).toEqual([
      '# Контекст поиска: how does rrf work',
      '',
      '> Черновик создан из pack_context. Это извлечённый контекст, а не сгенерированный ответ.',
      '',
      '## Запрос',
      '',
      'how  does',
      'rrf work',
      '',
      '## Параметры поиска',
      '',
      '- mode: lex',
      '- hits: 2 из 3',
      '- tokens: 120 из 2000',
      '',
      '## Упакованный контекст',
      '',
      'PACKED TEXT',
    ])
  })

  it('truncates long queries in the title and summary only', () => {
    const query = 'x'.repeat(200)
    const page = buildSearchContextPage(snapshotOf({ ...defaultSearchForm(), query }), packed, 2, 0)
    expect(page.title).toBe(`Контекст поиска: ${'x'.repeat(72)}`)
    expect(page.summary).toContain(`«${'x'.repeat(96)}»`)
    expect(page.content).toContain(query)
  })
})
