import { api } from '@/api/client'
import type { BacklinkItem, DocumentBody, WikiPutResult } from '@/api/types'
import { createRequestGate, errorMessage, type RequestTicket } from '@/lib/async'
import { ui } from './ui.svelte'
import type { WikiCatalog } from './wikiCatalog.svelte'
import type { WikiPrefs } from './wikiPrefs.svelte'

/** ASCII slug for wiki:// keys; mirrors gateway slugify defaults. */
export function slugifyTitle(title: string): string {
  let out = ''
  for (const c of title.trim()) {
    if (/[a-zA-Z0-9]/.test(c)) {
      out += c.toLowerCase()
    } else if (c === ' ' || c === '-' || c === '_') {
      if (out.length && !out.endsWith('-')) out += '-'
    }
  }
  out = out.replace(/^-+|-+$/g, '')
  return out || 'page'
}

/** Slug of a wiki document (`wiki://slug`), falling back to its id. */
function slugOf(doc: Pick<DocumentBody, 'uri' | 'id'>): string {
  return doc.uri.replace(/^wiki:\/\//, '') || doc.id
}

function isCasConflict(e: unknown): boolean {
  const msg = errorMessage(e)
  // api client throws `HTTP ${status}: …`; gateway may also say "conflict"
  return /\bHTTP 409\b/.test(msg) || /\bconflict\b/i.test(msg)
}

/**
 * The open wiki page and its editor: load, in-memory drafts per page, create, and
 * CAS saves (`if_match_revision` / `if_match_etag`) with conflict resolution.
 */
export class WikiPage {
  loading = $state(false)
  error = $state<string | null>(null)
  current = $state<DocumentBody | null>(null)
  backlinks = $state.raw<BacklinkItem[]>([])
  editing = $state(false)
  draftTitle = $state('')
  draftContent = $state('')
  dirty = $state(false)
  history = $state<string[]>([])
  /**
   * When set, openPage for this id enters edit mode (create flow).
   * Kept until cancel/save or navigation to another page so concurrent
   * route watchers (SideNav + WikiView) do not race out of the editor.
   */
  pendingEditId = $state<string | null>(null)
  creating = $state(false)
  saving = $state(false)

  /** One generation per opened (or closed) page. */
  private readonly gate = createRequestGate()
  private readonly drafts = new Map<string, { title: string; content: string }>()

  constructor(
    private readonly catalog: WikiCatalog,
    private readonly prefs: WikiPrefs,
  ) {}

  private rememberDraft() {
    if (this.current && this.editing && this.dirty) {
      this.drafts.set(this.current.id, { title: this.draftTitle, content: this.draftContent })
    }
  }

  async openPage(id: string, pushHistory = true) {
    this.rememberDraft()
    const ticket = this.gate.next()
    const previousId = this.current?.id ?? null
    this.loading = true
    this.error = null
    // Re-read after every await: a newer openPage / closePage may have superseded this one.
    const alive = () => ticket.current
    try {
      const doc = await api.document({ id })
      if (!alive()) return
      if (pushHistory && previousId && previousId !== id) this.history.push(previousId)
      this.rememberDraft()
      this.current = doc
      const draft = this.drafts.get(id)
      this.draftTitle = draft?.title ?? doc.title
      this.draftContent = draft?.content ?? doc.content
      this.dirty = Boolean(draft && (draft.title !== doc.title || draft.content !== doc.content))
      if (this.pendingEditId && this.pendingEditId !== id) {
        this.pendingEditId = null
      }
      this.editing = Boolean(draft) || this.pendingEditId === id
      this.prefs.touchRecent({ id: doc.id, title: doc.title, slug: slugOf(doc) })
      try {
        const bl = await api.backlinks(id)
        if (!alive()) return
        this.backlinks = bl.backlinks
      } catch {
        if (alive()) this.backlinks = []
      }
    } catch (e) {
      if (ticket.current) {
        this.error = errorMessage(e)
        this.current = null
      }
    } finally {
      if (ticket.current) this.loading = false
    }
  }

  goBack() {
    const prev = this.history.pop()
    if (prev) void this.openPage(prev, false)
  }

  /** Leave the open page (route to /wiki root shows the home dashboard). */
  closePage() {
    this.rememberDraft()
    this.gate.cancel()
    this.loading = false
    this.current = null
    this.backlinks = []
    this.editing = false
    this.dirty = false
    this.pendingEditId = null
    this.error = null
  }

  startEdit() {
    if (!this.current || this.saving || this.editing) return
    this.draftTitle = this.current.title
    this.draftContent = this.current.content
    this.editing = true
    this.dirty = false
  }

  cancelEdit() {
    if (this.saving) return
    this.editing = false
    this.dirty = false
    this.pendingEditId = null
    if (this.current) {
      this.drafts.delete(this.current.id)
      this.draftTitle = this.current.title
      this.draftContent = this.current.content
    }
  }

  markDirty() {
    this.dirty = true
  }

  /**
   * Prompt for a title (unless given), POST /v1/wiki to create, refresh catalog.
   * Returns the new document id; caller should route to /wiki/:id so openPage
   * enters the editor via pendingEditId.
   */
  async createPage(title?: string): Promise<string | null> {
    if (this.creating || this.saving) return null
    let t = title?.trim() ?? ''
    if (!t) {
      const raw = window.prompt(ui.t('createPrompt'))
      if (raw == null) return null
      t = raw.trim()
    }
    if (!t) {
      ui.toast(ui.t('createTitleRequired'), 'error')
      return null
    }
    const slug = this.catalog.uniqueSlug(slugifyTitle(t))
    this.creating = true
    try {
      const res = await api.createWiki({
        slug,
        title: t,
        content: `# ${t}\n\n`,
        kind: 'wiki',
      })
      const id = res.document_id
      if (!id) {
        ui.toast(ui.t('createNoId'), 'error')
        return null
      }
      this.pendingEditId = id
      // Clear filter/facet so the new page is visible in the catalog list.
      this.catalog.clearScope()
      await this.catalog.load()
      this.prefs.touchRecent({ id, title: t, slug })
      ui.toast(ui.t('pageCreated'), 'ok')
      return id
    } catch (e) {
      if (isCasConflict(e)) {
        ui.toast(
          ui.locale === 'ru'
            ? 'Страница с таким адресом уже существует. Выберите другое название.'
            : 'A page with this address already exists. Choose another title.',
          'error',
        )
        return null
      }
      ui.toast(errorMessage(e), 'error')
      throw e
    } finally {
      this.creating = false
    }
  }

  private isCurrentPage(id: string, ticket: RequestTicket): boolean {
    return ticket.current && this.current?.id === id
  }

  /** CAS write of a draft against the revision / ETag the editor started from. */
  private writeSnapshot(cur: DocumentBody, title: string, content: string) {
    return api.putWiki({
      slug: slugOf(cur),
      id: cur.id,
      uri: cur.uri,
      title,
      content,
      ...(cur.revision != null ? { if_match_revision: cur.revision } : {}),
      ...(cur.etag != null ? { if_match_etag: cur.etag } : {}),
    })
  }

  /** Advance only the saved baseline; newer keystrokes remain an editable draft. */
  private applySaved(
    cur: DocumentBody,
    ticket: RequestTicket,
    title: string,
    content: string,
    res: WikiPutResult,
  ) {
    const cached = this.drafts.get(cur.id)
    if (cached?.title === title && cached.content === content) this.drafts.delete(cur.id)
    if (!this.isCurrentPage(cur.id, ticket)) return
    this.current = { ...cur, title, content, revision: res.revision, etag: res.etag }
    this.pendingEditId = null
    this.dirty = this.draftTitle !== title || this.draftContent !== content
    this.editing = this.dirty
    if (this.dirty) this.rememberDraft()
    else this.drafts.delete(cur.id)
    this.prefs.touchRecent({ id: cur.id, title, slug: res.slug })
  }

  /** A conflict dialog belongs only to the page where this save started. */
  private async handleCasConflict(cur: DocumentBody, ticket: RequestTicket) {
    if (!this.isCurrentPage(cur.id, ticket)) return
    ui.toast(ui.t('saveConflictToast'), 'error')
    const remote = await api.document({ id: cur.id })
    if (!this.isCurrentPage(cur.id, ticket)) return
    const keepDraft = window.confirm(
      [
        ui.t('saveConflictTitle'),
        '',
        ui.t('saveConflictRemote', { rev: remote.revision ?? '?', title: remote.title }),
        ui.t('saveConflictDraft', { title: this.draftTitle }),
        '',
        `OK - ${ui.t('saveConflictKeep')}`,
        `Cancel - ${ui.t('saveConflictDiscard')}`,
      ].join('\n'),
    )
    if (!this.isCurrentPage(cur.id, ticket)) return
    this.current = remote
    if (!keepDraft) {
      this.drafts.delete(cur.id)
      this.pendingEditId = null
      this.draftTitle = remote.title
      this.draftContent = remote.content
      this.dirty = false
      this.editing = false
      try {
        const bl = await api.backlinks(cur.id)
        if (this.isCurrentPage(cur.id, ticket)) this.backlinks = bl.backlinks
      } catch {
        if (this.isCurrentPage(cur.id, ticket)) this.backlinks = []
      }
      ui.toast(ui.t('saveConflictReloaded'), 'info')
      return
    }

    // Snapshot the latest draft after the user explicitly chose to re-save.
    const title = this.draftTitle
    const content = this.draftContent
    this.editing = true
    this.dirty = true
    try {
      const res = await this.writeSnapshot(remote, title, content)
      this.applySaved(remote, ticket, title, content, res)
      await this.catalog.load()
      ui.toast(ui.t('saveConflictSavedOver'), 'ok')
    } catch (cause) {
      if (isCasConflict(cause)) ui.toast(ui.t('saveConflictStillFailing'), 'error')
      throw cause
    }
  }

  async save() {
    const cur = this.current
    if (!cur || this.saving || !this.editing || !this.dirty) return
    const ticket = this.gate.latest
    const title = this.draftTitle
    const content = this.draftContent
    this.saving = true
    try {
      try {
        const res = await this.writeSnapshot(cur, title, content)
        this.applySaved(cur, ticket, title, content, res)
        await this.catalog.load()
        ui.toast(ui.t('saved'), 'ok')
      } catch (cause) {
        if (!isCasConflict(cause)) throw cause
        await this.handleCasConflict(cur, ticket)
      }
    } catch (cause) {
      ui.toast(errorMessage(cause), 'error')
      throw cause
    } finally {
      this.saving = false
    }
  }
}
