import type { BacklinkItem, DocumentBody, WikiListParams, WikiPageMeta } from '@/api/types'
import { WikiCatalog, type CatalogFacet } from './wikiCatalog.svelte'
import { WikiPage } from './wikiPage.svelte'
import { WikiPrefs, type FavoritePage, type RecentPage } from './wikiPrefs.svelte'

export { slugifyTitle } from './wikiPage.svelte'
export type { CatalogFacet, FavoritePage, RecentPage }

/**
 * The wiki screen's state, composed of three stores:
 *
 * - `catalog` — page list, sidebar filter and facet (`wikiCatalog.svelte.ts`)
 * - `page`    — the open page, its editor and CAS saves (`wikiPage.svelte.ts`)
 * - `prefs`   — favorites, recents and scroll positions in browser storage
 *               (`wikiPrefs.svelte.ts`)
 *
 * The flat accessors below keep the long-standing `wiki.current` / `wiki.pages`
 * call sites working; new code can reach for the sub-store it needs.
 */
export class WikiStore {
  readonly prefs = new WikiPrefs()
  readonly catalog = new WikiCatalog((pages) => {
    this.prefs.syncFromCatalog(pages)
  })
  readonly page = new WikiPage(this.catalog, this.prefs)

  readonly loading = $derived(this.catalog.loading || this.page.loading)
  /** Favorites for SideNav; catalog meta preferred when the page is loaded. */
  readonly favoritePages = $derived(this.prefs.favoritePagesFor(this.catalog.pages))

  /** Start background reactions (debounced filter reload). Returns the stop function. */
  start(): () => void {
    return this.catalog.start()
  }

  /* ---------- catalog ---------- */

  get pages(): WikiPageMeta[] {
    return this.catalog.pages
  }
  set pages(value: WikiPageMeta[]) {
    this.catalog.pages = value
  }
  get filter(): string {
    return this.catalog.filter
  }
  set filter(value: string) {
    this.catalog.filter = value
  }
  get facet(): CatalogFacet {
    return this.catalog.facet
  }
  set facet(value: CatalogFacet) {
    this.catalog.facet = value
  }
  get categories(): string[] {
    return this.catalog.categories
  }
  get filtered(): WikiPageMeta[] {
    return this.catalog.filtered
  }
  /** Catalog and page failures stay separate so one request cannot mislabel another. */
  get catalogError(): string | null {
    return this.catalog.error
  }
  get catalogQ(): string | null {
    return this.catalog.catalogQ
  }
  get catalogTotal(): number | null {
    return this.catalog.total
  }
  get catalogHasMore(): boolean {
    return this.catalog.hasMore
  }
  get loadingMore(): boolean {
    return this.catalog.loadingMore
  }
  get loadMoreError(): string | null {
    return this.catalog.loadMoreError
  }
  setFacet(next: CatalogFacet) {
    this.catalog.setFacet(next)
  }
  facetIsAll(): boolean {
    return this.catalog.facetIsAll()
  }
  facetIsKind(kind: string): boolean {
    return this.catalog.facetIsKind(kind)
  }
  facetIsCategory(category: string): boolean {
    return this.catalog.facetIsCategory(category)
  }
  loadCatalog(params?: WikiListParams): Promise<void> {
    return this.catalog.load(params)
  }
  loadMore(): Promise<void> {
    return this.catalog.loadMore()
  }

  /* ---------- open page + editor ---------- */

  get current(): DocumentBody | null {
    return this.page.current
  }
  set current(value: DocumentBody | null) {
    this.page.current = value
  }
  get pageError(): string | null {
    return this.page.error
  }
  get backlinks(): BacklinkItem[] {
    return this.page.backlinks
  }
  set backlinks(value: BacklinkItem[]) {
    this.page.backlinks = value
  }
  get editing(): boolean {
    return this.page.editing
  }
  set editing(value: boolean) {
    this.page.editing = value
  }
  get draftTitle(): string {
    return this.page.draftTitle
  }
  set draftTitle(value: string) {
    this.page.draftTitle = value
  }
  get draftContent(): string {
    return this.page.draftContent
  }
  set draftContent(value: string) {
    this.page.draftContent = value
  }
  get dirty(): boolean {
    return this.page.dirty
  }
  set dirty(value: boolean) {
    this.page.dirty = value
  }
  get history(): string[] {
    return this.page.history
  }
  set history(value: string[]) {
    this.page.history = value
  }
  get pendingEditId(): string | null {
    return this.page.pendingEditId
  }
  set pendingEditId(value: string | null) {
    this.page.pendingEditId = value
  }
  get creating(): boolean {
    return this.page.creating
  }
  get saving(): boolean {
    return this.page.saving
  }
  set saving(value: boolean) {
    this.page.saving = value
  }
  openPage(id: string, pushHistory = true): Promise<void> {
    return this.page.openPage(id, pushHistory)
  }
  goBack() {
    this.page.goBack()
  }
  closePage() {
    this.page.closePage()
  }
  startEdit() {
    this.page.startEdit()
  }
  cancelEdit() {
    this.page.cancelEdit()
  }
  markDirty() {
    this.page.markDirty()
  }
  createPage(title?: string): Promise<string | null> {
    return this.page.createPage(title)
  }
  save(): Promise<void> {
    return this.page.save()
  }

  /* ---------- browser-local preferences ---------- */

  get recent(): RecentPage[] {
    return this.prefs.recent
  }
  get favorites(): FavoritePage[] {
    return this.prefs.favorites
  }
  set favorites(value: FavoritePage[]) {
    this.prefs.favorites = value
  }
  isFavorite(id: string): boolean {
    return this.prefs.isFavorite(id)
  }
  toggleFavorite(id: string) {
    this.prefs.toggleFavorite(
      id,
      this.catalog.pages.find((p) => p.id === id),
      this.page.current,
    )
  }
  touchRecent(page: RecentPage) {
    this.prefs.touchRecent(page)
  }
  saveScrollPosition(pageId: string, top: number) {
    this.prefs.saveScrollPosition(pageId, top)
  }
  getScrollPosition(pageId: string): number {
    return this.prefs.getScrollPosition(pageId)
  }
  clearScrollPosition(pageId: string) {
    this.prefs.clearScrollPosition(pageId)
  }
}

export const wiki = new WikiStore()
