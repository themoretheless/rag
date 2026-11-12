// @vitest-environment jsdom
import { render, screen, waitFor } from '@testing-library/svelte'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import CorpusView from './CorpusView.svelte'

vi.mock('@/api/client', async () => {
  const { documentsQuery } =
    await vi.importActual<typeof import('@/api/endpoints/corpus')>('@/api/endpoints/corpus')
  const { loadPanels } = await vi.importActual<typeof import('@/lib/async')>('@/lib/async')
  return {
    documentsQuery,
    loadPanels,
    api: {
      documents: vi.fn(),
      multiGet: vi.fn(),
      backlinks: vi.fn(),
      projects: vi.fn(),
      taxonomy: vi.fn(),
      embeddingManifest: vi.fn(),
      projectHome: vi.fn(),
      sourceFile: vi.fn(),
      startSync: vi.fn(),
    },
  }
})
vi.mock('@/lib/router.svelte', () => ({
  goGraph: vi.fn(),
  route: { name: 'corpus', query: new URLSearchParams('project=rag&q=spec') },
}))
vi.mock('@/lib/state/ui.svelte', () => ({ ui: { toast: vi.fn() } }))

const mocked = vi.mocked(api)

beforeEach(() => {
  vi.clearAllMocks()
  mocked.documents.mockResolvedValue({
    items: [
      {
        id: 'd1',
        uri: 'file:///d1',
        title: 'Spec',
        layer: 'raw',
        kind: 'document',
        status: 'active',
        updated_at: '',
      },
    ],
    page: { total: 1, next_cursor: null },
  })
  mocked.multiGet.mockResolvedValue([])
  mocked.backlinks.mockResolvedValue({ ok: true, count: 0, backlinks: [] })
  mocked.projects.mockResolvedValue([{ project_id: 'rag', document_count: 1 }])
  mocked.taxonomy.mockResolvedValue(undefined)
  mocked.embeddingManifest.mockResolvedValue({})
})

describe('CorpusView', () => {
  it('requests each panel exactly once on mount, scoped by the URL', async () => {
    render(CorpusView)
    await screen.findAllByText('Spec')
    await waitFor(() => {
      expect(mocked.embeddingManifest).toHaveBeenCalledTimes(1)
    })
    expect(mocked.documents).toHaveBeenCalledTimes(1)
    expect(mocked.documents).toHaveBeenCalledWith(
      expect.objectContaining({ limit: 200, wing: 'rag', q: 'spec' }),
      expect.anything(),
    )
    expect(mocked.projects).toHaveBeenCalledTimes(1)
    expect(mocked.taxonomy).toHaveBeenCalledTimes(1)
  })
})
