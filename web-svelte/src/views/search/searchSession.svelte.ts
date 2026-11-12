import { api } from '@/api/client'
import type { PackContextResponse, SearchHit, SearchRequest, SearchResponse } from '@/api/types'
import { createRequestGate, errorMessage } from '@/lib/async'
import {
  buildSearchContextPage,
  defaultSearchForm,
  packRequest,
  searchRequest,
  snapshotOf,
  type SearchForm,
  type SearchSnapshot,
} from './searchModel'

interface PackedSnapshot {
  search: SearchSnapshot
  sourceHitCount: number
}

/**
 * State of one search-lab screen: the editable form, the last executed search and
 * the context packed from it. Results and packed context remember the parameter
 * snapshot that produced them, so the screen can tell when the form has moved on.
 */
export class SearchSession {
  form = $state<SearchForm>(defaultSearchForm())
  busy = $state(false)
  packing = $state(false)
  creatingWiki = $state(false)
  error = $state('')
  /** Replaced wholesale per search; hits are never mutated, so no deep proxy. */
  response = $state.raw<SearchResponse | null>(null)
  packed = $state.raw<PackContextResponse | null>(null)
  /** Index of the highlighted hit in `hits`. */
  selected = $state(0)
  resultSnapshot = $state.raw<SearchSnapshot | null>(null)
  private packedSnapshot = $state.raw<PackedSnapshot | null>(null)

  private readonly searchGate = createRequestGate()
  private readonly packGate = createRequestGate()

  private readonly formSignature = $derived(snapshotOf(this.form).signature)

  readonly hits = $derived<SearchHit[]>(this.response?.items ?? [])
  // Response-level timings are the only timing source used here. Per-hit
  // explanations describe the same search, but mixing both levels can make
  // the summary silently change when the selected result changes.
  readonly timing = $derived(this.response?.timings ?? null)
  readonly selectedHit = $derived(this.hits[this.selected] ?? null)
  /** Mode of the shown results, or of the form before the first search. */
  readonly mode = $derived(this.response?.mode ?? this.form.mode)
  /** The form no longer matches the parameters of the shown results. */
  readonly resultsDirty = $derived(
    this.resultSnapshot !== null && this.resultSnapshot.signature !== this.formSignature,
  )
  /** The packed context no longer matches the form, the results, or their hit count. */
  readonly packedDirty = $derived(
    this.packedSnapshot !== null &&
      (this.packedSnapshot.search.signature !== this.formSignature ||
        this.packedSnapshot.search.signature !== this.resultSnapshot?.signature ||
        this.packedSnapshot.sourceHitCount !== this.hits.length),
  )
  readonly appliedRrfK = $derived(this.timing?.rrf_k ?? this.resultSnapshot?.rrfK ?? this.form.rrfK)
  /** Hits the packed context was built from; null before packing. */
  readonly packedSourceCount = $derived(this.packedSnapshot?.sourceHitCount ?? null)
  /** Request body of the shown results, as saved with a relevance assessment. */
  readonly executedRequest = $derived<SearchRequest | null>(
    this.resultSnapshot ? searchRequest(this.resultSnapshot) : null,
  )

  /** Run the search for the current form. A newer run supersedes an older one. */
  async run(): Promise<void> {
    const snapshot = snapshotOf(this.form)
    if (!snapshot.query) return
    const ticket = this.searchGate.next()
    this.packGate.cancel()
    this.busy = true
    this.packing = false
    this.error = ''
    this.response = null
    this.resultSnapshot = null
    this.packed = null
    this.packedSnapshot = null
    this.selected = 0
    try {
      const next = await api.search(searchRequest(snapshot), { signal: ticket.signal })
      if (!ticket.current) return
      this.response = next
      this.resultSnapshot = snapshot
    } catch (cause) {
      if (ticket.current) this.error = errorMessage(cause)
    } finally {
      if (ticket.current) this.busy = false
    }
  }

  /** Pack the shown hits into agent context. Refused while the form is ahead of the results. */
  async pack(): Promise<void> {
    const search = this.resultSnapshot
    if (!this.hits.length || !search || this.resultsDirty) return
    const ticket = this.packGate.next()
    const sourceHits = [...this.hits]
    this.packing = true
    this.error = ''
    try {
      const next = await api.packContext(packRequest(search, sourceHits), { signal: ticket.signal })
      if (!ticket.current) return
      this.packed = next
      this.packedSnapshot = { search, sourceHitCount: sourceHits.length }
    } catch (cause) {
      if (ticket.current) this.error = errorMessage(cause)
    } finally {
      if (ticket.current) this.packing = false
    }
  }

  /**
   * Save the packed context as a new wiki page. Resolves to the new document id,
   * `null` when the request failed (`error` is set), or `undefined` when there is
   * nothing current to save.
   */
  async createWikiDraft(): Promise<string | null | undefined> {
    const packed = this.packed
    const packedSnapshot = this.packedSnapshot
    if (!packed?.context_text || !packedSnapshot || this.packedDirty || this.creatingWiki) return undefined
    this.creatingWiki = true
    this.error = ''
    try {
      const page = buildSearchContextPage(
        packedSnapshot.search,
        packed,
        packedSnapshot.sourceHitCount,
        Date.now(),
      )
      return (await api.createWiki(page)).document_id
    } catch (cause) {
      this.error = errorMessage(cause)
      return null
    } finally {
      this.creatingWiki = false
    }
  }
}

export function createSearchSession(): SearchSession {
  return new SearchSession()
}
