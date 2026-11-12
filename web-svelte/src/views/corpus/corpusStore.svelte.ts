import { api, documentsQuery, loadPanels } from '@/api/client'
import type {
  BacklinkItem,
  DocumentDetail,
  DocumentRow,
  EmbeddingManifestStatus,
  ProjectHome,
  ProjectSummary,
  TaxonomySummary,
} from '@/api/types'
import { createRequestGate, errorMessage, type RequestTicket } from '@/lib/async'
import { ui } from '@/lib/state/ui.svelte'
import { documentFilters, loadDocumentDetail, mergeRows, prependRow, type CorpusFilters } from './corpusModel'

/** Scope and deep-link target taken from the corpus URL. */
export interface CorpusRoute {
  project: string
  room: string
  query: string
  /** Document to select once the catalog is loaded; '' for none. */
  documentId: string
}

/**
 * Corpus screen state: scope filters, the cursor-paginated catalog, the selected
 * document with its detail, and the side panels (projects, taxonomy, embeddings).
 */
export class CorpusStore implements CorpusFilters {
  project = $state('')
  room = $state('')
  layer = $state('')
  status = $state('')
  query = $state('')

  /** Loaded catalog rows; replaced on every page, never mutated. */
  rows = $state.raw<DocumentRow[]>([])
  total = $state(0)
  nextCursor = $state<string | null>(null)
  busy = $state(false)
  loadingMore = $state(false)
  documentError = $state('')
  appendError = $state('')

  selected = $state.raw<DocumentRow | null>(null)
  detail = $state.raw<DocumentDetail | null>(null)
  backlinks = $state.raw<BacklinkItem[]>([])
  detailBusy = $state(false)
  detailError = $state('')
  /** The full-text dialog of the selected document is open. */
  textOpen = $state(false)

  projects = $state.raw<ProjectSummary[]>([])
  taxonomy = $state.raw<Partial<TaxonomySummary>>({})
  embeddingStatus = $state.raw<EmbeddingManifestStatus | null>(null)
  projectHome = $state.raw<ProjectHome | null>(null)
  panelWarning = $state('')

  /** Query signature of the loaded rows; '' before the first page. */
  private appliedSignature = $state('')
  /** Deep-linked document still waiting for the catalog to load. */
  private pendingSelectionId = ''

  private readonly catalogGate = createRequestGate()
  private readonly detailGate = createRequestGate()
  private readonly homeGate = createRequestGate()
  private readonly refreshGate = createRequestGate()

  private readonly signature = $derived(documentsQuery(documentFilters(this)))

  readonly detailDocument = $derived(this.detail?.document ?? null)
  readonly allDocumentCount = $derived(
    this.taxonomy.total_documents ??
      this.projects.reduce((sum, item) => sum + item.document_count, 0) + (this.taxonomy.unscoped_count ?? 0),
  )
  /** The filters no longer match the loaded rows, so "load more" would mix two queries. */
  readonly filtersDirty = $derived(Boolean(this.appliedSignature) && this.signature !== this.appliedSignature)

  /** Take scope and deep-link target from the URL. Does not load anything. */
  applyRoute(route: CorpusRoute): void {
    this.project = route.project
    this.room = route.room
    this.query = route.query
    this.pendingSelectionId = route.documentId
  }

  /**
   * Load the first catalog page for the current filters, or (`append`) the next cursor
   * page of the query that is already loaded. `refresh` ties the request to a
   * `refreshAll()` run so a superseded run cannot apply its results.
   */
  async loadDocuments(append = false, refresh?: RequestTicket): Promise<void> {
    const cursor = append ? this.nextCursor : null
    const filters = documentFilters(this)
    const requestSignature = documentsQuery(filters)
    if (append && (!cursor || requestSignature !== this.appliedSignature)) return
    const ticket = this.catalogGate.next()
    const alive = () => ticket.current && (!refresh || refresh.current)
    if (append) {
      this.loadingMore = true
      this.appendError = ''
    } else {
      this.busy = true
      this.documentError = ''
      this.appendError = ''
    }
    try {
      const response = await api.documents(cursor ? { ...filters, cursor } : filters, {
        signal: ticket.signal,
      })
      if (!alive()) return
      if (append && requestSignature !== this.signature) return
      const incoming = response.items ?? []
      this.rows = append ? mergeRows(this.rows, incoming) : incoming
      this.total = response.page?.total ?? this.rows.length
      this.nextCursor = response.page?.next_cursor ?? null
      if (!append) this.appliedSignature = requestSignature
      this.selectAfterPage(ticket, append)
    } catch (cause) {
      if (!alive()) return
      const message = errorMessage(cause)
      if (append) this.appendError = message
      else this.documentError = message
    } finally {
      if (alive()) {
        this.busy = false
        this.loadingMore = false
      }
    }
  }

  loadMore(): Promise<void> {
    return this.loadDocuments(true)
  }

  /**
   * Selection after a page has been applied. A fresh first page drops a selection that
   * is no longer listed and honours a pending deep link. Whenever nothing is selected
   * (including after "load more" with the inspector closed) the first row is opened.
   */
  private selectAfterPage(catalog: RequestTicket, append: boolean): void {
    const selectedId = this.selected?.id
    if (!append && selectedId !== undefined && !this.rows.some((row) => row.id === selectedId)) {
      this.selected = null
      this.detail = null
    }
    const targetId = append ? '' : this.pendingSelectionId
    if (targetId) {
      const target = this.rows.find((row) => row.id === targetId)
      if (target) {
        this.pendingSelectionId = ''
        void this.choose(target)
      } else {
        void this.loadRoutedDocument(targetId, catalog)
      }
    } else if (!this.selected) {
      const first = this.rows[0]
      if (first) void this.choose(first)
    }
  }

  /** A deep-linked document outside the first page is fetched on its own and listed first. */
  private async loadRoutedDocument(documentId: string, catalog: RequestTicket): Promise<void> {
    const wanted = () => catalog.current && this.pendingSelectionId === documentId
    try {
      const response = await api.multiGet([documentId], { include_chunks: false })
      if (!wanted()) return
      this.pendingSelectionId = ''
      const routed = response[0]?.document
      if (!routed) {
        this.panelWarning = `Документ ${documentId} не найден`
        return
      }
      this.rows = prependRow(this.rows, routed)
      void this.choose(routed)
    } catch (cause) {
      if (!wanted()) return
      this.pendingSelectionId = ''
      this.panelWarning = `Не удалось открыть документ по ссылке: ${errorMessage(cause)}`
    }
  }

  /** Reload every panel. A failed panel is reported without hiding the others. */
  async refreshAll(): Promise<void> {
    const refresh = this.refreshGate.next()
    const selectedAtStart = this.selected?.id ?? ''
    this.panelWarning = ''
    const failures = await loadPanels([
      [
        'проекты',
        async () => {
          const projects = await api.projects()
          if (refresh.current) this.projects = projects
        },
      ],
      ['документы', () => this.loadDocuments(false, refresh)],
      [
        'taxonomy',
        async () => {
          const taxonomy = await api.taxonomy()
          if (refresh.current) this.taxonomy = taxonomy ?? {}
        },
      ],
      [
        'манифест эмбеддингов',
        async () => {
          const status = await api.embeddingManifest()
          if (refresh.current) this.embeddingStatus = status
        },
      ],
    ])
    if (!refresh.current) return
    if (failures.length) this.panelWarning = `Не обновлены панели: ${failures.join(', ')}`
    // The catalog reload leaves an unchanged selection alone; refresh its detail too.
    const selected = this.selected
    if (selectedAtStart && selected?.id === selectedAtStart) await this.choose(selected, refresh)
  }

  /** Select a row and load its detail for the inspector. */
  async choose(row: DocumentRow, refresh?: RequestTicket): Promise<void> {
    const ticket = this.detailGate.next()
    const alive = () => ticket.current && this.selected?.id === row.id && (!refresh || refresh.current)
    this.selected = row
    this.detail = null
    this.backlinks = []
    this.textOpen = false
    this.detailBusy = true
    this.detailError = ''
    try {
      const bundle = await loadDocumentDetail(row.id)
      if (!alive()) return
      this.detail = bundle.detail
      this.backlinks = bundle.backlinks
    } catch (cause) {
      if (!alive()) return
      this.detailError = errorMessage(cause)
      ui.toast(this.detailError, 'error')
    } finally {
      if (alive()) this.detailBusy = false
    }
  }

  closeInspector(): void {
    this.detailGate.cancel()
    this.selected = null
    this.detail = null
    this.backlinks = []
    this.detailBusy = false
    this.detailError = ''
    this.textOpen = false
  }

  /** Narrow the catalog to a project (wing) and optionally one of its rooms. */
  chooseScope(project: string, room = ''): void {
    const ticket = this.homeGate.next()
    this.project = project
    this.room = room
    this.projectHome = null
    if (project) {
      api.projectHome(project).then(
        (home) => {
          if (ticket.current && this.project === project && this.room === room) this.projectHome = home
        },
        (cause: unknown) => {
          if (!ticket.current) return
          console.warn('project-home load failed', cause)
          this.panelWarning = `Не удалось загрузить корни проекта: ${errorMessage(cause)}`
        },
      )
    }
    void this.loadDocuments()
  }

  /** Queue a `sync_sources` job for the current scope; resolves to the job id. */
  startSync(path: string): Promise<string> {
    return api.startSync({ path, wing: this.project || undefined, room: this.room || undefined })
  }
}

export function createCorpusStore(): CorpusStore {
  return new CorpusStore()
}
