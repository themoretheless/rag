/** UI label keys for shell + wiki (en/ru). Message catalog is locale-agnostic. */
export type Locale = 'en' | 'ru'

export type MessageKey =
  | 'toggleSidebar'
  | 'wiki'
  | 'graph'
  | 'search'
  | 'theme'
  | 'offline'
  | 'healthStats'
  | 'lang'
  | 'pages'
  | 'favorites'
  | 'pinFavorite'
  | 'unpinFavorite'
  | 'refresh'
  | 'newPage'
  | 'filterPages'
  | 'filterFacets'
  | 'facetAll'
  | 'loading'
  | 'noPages'
  | 'back'
  | 'showInGraph'
  | 'edit'
  | 'cancel'
  | 'save'
  | 'saveFailed'
  | 'untitled'
  | 'homeGreeting'
  | 'homeSubtitle'
  | 'openSearch'
  | 'openGraph'
  | 'allPages'
  | 'recentPages'
  | 'offlineTitle'
  | 'offlineHint'
  | 'retry'
  | 'emptyWikiTitle'
  | 'emptyWikiHint'
  | 'createFirstPage'
  | 'noPagesHint'
  | 'writeMarkdown'
  | 'casHint'
  | 'backlinks'
  | 'incomingWikilinks'
  | 'noIncoming'
  | 'properties'
  | 'kind'
  | 'layer'
  | 'revision'
  | 'updated'
  | 'outline'
  | 'history'
  | 'breadcrumb'
  | 'palettePlaceholder'
  | 'paletteNoMatches'
  | 'paletteNav'
  | 'paletteOpen'
  | 'paletteClose'
  | 'paletteActionNew'
  | 'paletteActionGraph'
  | 'paletteActionSearch'
  | 'searchHeading'
  | 'searchSubtitle'
  | 'searchPlaceholder'
  | 'searchNoResults'
  | 'graphNodes'
  | 'graphEdges'
  | 'graphFull'
  | 'graphExpand'
  | 'graphSeedPlaceholder'
  | 'graphDepth'
  | 'graphMax'
  | 'graphTags'
  | 'graphFit'
  | 'graphFocus'
  | 'graphLoading'
  | 'graphNoNodes'
  | 'graphOpenAsWiki'
  | 'graphMinimapHint'
  | 'graphEmptyMinimap'
  | 'layoutForce'
  | 'layoutRadial'
  | 'createPrompt'
  | 'createTitleRequired'
  | 'createNoId'
  | 'pageCreated'
  | 'saved'
  | 'saveConflictToast'
  | 'saveConflictTitle'
  | 'saveConflictRemote'
  | 'saveConflictDraft'
  | 'saveConflictKeep'
  | 'saveConflictDiscard'
  | 'saveConflictReloadFail'
  | 'saveConflictReloaded'
  | 'saveConflictSavedOver'
  | 'saveConflictStillFailing'
  | 'confirm'
  | 'keepDraft'
  | 'discardReload'

export type Messages = Record<MessageKey, string>

export const messages: Record<Locale, Messages> = {
  en: {
    toggleSidebar: 'Toggle sidebar (⌘B)',
    wiki: 'Wiki',
    graph: 'Graph',
    search: 'Search',
    theme: 'Theme',
    offline: 'offline',
    healthStats: '{docs} docs · {nodes} nodes',
    lang: 'Language',
    pages: 'Pages',
    favorites: 'Favorites',
    pinFavorite: 'Pin to favorites',
    unpinFavorite: 'Unpin from favorites',
    refresh: 'Refresh',
    newPage: 'New page',
    filterPages: 'Filter pages…',
    filterFacets: 'Filter pages by kind or category',
    facetAll: 'All',
    loading: 'Loading…',
    noPages: 'No pages',
    back: '← Back',
    showInGraph: 'Show in graph',
    edit: 'Edit',
    cancel: 'Cancel',
    save: 'Save',
    saveFailed: 'Save failed',
    untitled: 'Untitled',
    homeGreeting: 'Welcome to the wiki',
    homeSubtitle: 'Your compiled knowledge base. Pick a page below or start a new one.',
    openSearch: 'Search',
    openGraph: 'Open graph',
    allPages: 'All pages',
    recentPages: 'Recent',
    offlineTitle: 'Gateway is offline',
    offlineHint: 'Start the HTTP gateway, then retry:',
    retry: 'Retry',
    emptyWikiTitle: 'The wiki is empty',
    emptyWikiHint: 'Create your first page here, or ingest documents via the rag-mcp tools.',
    createFirstPage: 'Create the first page',
    noPagesHint: 'Create a page with the + button above.',
    writeMarkdown: 'Write markdown… Use [[wikilinks]] and #tags',
    casHint: 'CAS: save sends if_match_revision from last load. Conflict → re-open and merge.',
    backlinks: 'Backlinks',
    incomingWikilinks: 'Incoming [[wikilinks]]',
    noIncoming: 'No incoming links',
    properties: 'Properties',
    kind: 'Kind',
    layer: 'Layer',
    revision: 'Revision',
    updated: 'Updated',
    outline: 'Outline',
    history: 'History',
    breadcrumb: 'Breadcrumb',
    palettePlaceholder: 'Jump to page or run an action…',
    paletteNoMatches: 'No matches',
    paletteNav: '↑↓ navigate',
    paletteOpen: '↵ open',
    paletteClose: 'esc close',
    paletteActionNew: 'New page',
    paletteActionGraph: 'Open graph',
    paletteActionSearch: 'Open search',
    searchHeading: 'Search',
    searchSubtitle: 'Notion-style jump across wiki catalog + graph find.',
    searchPlaceholder: 'Query…',
    searchNoResults: 'No results yet',
    graphNodes: '{count} nodes',
    graphEdges: '{count} edges',
    graphFull: 'Full graph',
    graphExpand: 'Expand seed',
    graphSeedPlaceholder: 'Seed id / label / document_id',
    graphDepth: 'depth',
    graphMax: 'max',
    graphTags: 'tags',
    graphFit: 'Fit view',
    graphFocus: 'Focus neighbors',
    graphLoading: 'Loading graph…',
    graphNoNodes: 'No nodes',
    graphOpenAsWiki: 'Open as wiki',
    graphMinimapHint: 'Overview (click to navigate)',
    graphEmptyMinimap: 'empty',
    layoutForce: 'Force',
    layoutRadial: 'Radial',
    createPrompt: 'New page title',
    createTitleRequired: 'Title is required',
    createNoId: 'Create succeeded but no document_id returned',
    pageCreated: 'Page created',
    saved: 'Saved',
    saveConflictToast: 'CAS conflict (409): page was updated elsewhere. Your draft is still in the editor.',
    saveConflictTitle: 'Save conflict: this page was changed by another writer (CAS 409).',
    saveConflictRemote: 'Remote: r{rev} - "{title}"',
    saveConflictDraft: 'Your draft: "{title}"',
    saveConflictKeep: 'Keep your draft and save again using the remote revision',
    saveConflictDiscard: 'Discard draft and reload the remote page',
    saveConflictReloadFail: 'Could not reload after conflict: {msg}',
    saveConflictReloaded: 'Reloaded remote page; draft discarded',
    saveConflictSavedOver: 'Saved over remote revision',
    saveConflictStillFailing: 'Still conflicting after retry. Reload remote or merge manually, then save again.',
    confirm: 'Confirm',
    keepDraft: 'Keep draft',
    discardReload: 'Discard & reload',
  },
  ru: {
    toggleSidebar: 'Боковая панель (⌘B)',
    wiki: 'Вики',
    graph: 'Граф',
    search: 'Поиск',
    theme: 'Тема',
    offline: 'офлайн',
    healthStats: '{docs} док. · {nodes} узлов',
    lang: 'Язык',
    pages: 'Страницы',
    favorites: 'Избранное',
    pinFavorite: 'В избранное',
    unpinFavorite: 'Убрать из избранного',
    refresh: 'Обновить',
    newPage: 'Новая страница',
    filterPages: 'Фильтр страниц…',
    filterFacets: 'Фильтр по типу или категории',
    facetAll: 'Все',
    loading: 'Загрузка…',
    noPages: 'Нет страниц',
    back: '← Назад',
    showInGraph: 'В графе',
    edit: 'Правка',
    cancel: 'Отмена',
    save: 'Сохранить',
    saveFailed: 'Не удалось сохранить',
    untitled: 'Без названия',
    homeGreeting: 'Добро пожаловать в вики',
    homeSubtitle: 'Ваша собранная база знаний. Выберите страницу ниже или создайте новую.',
    openSearch: 'Поиск',
    openGraph: 'Открыть граф',
    allPages: 'Все страницы',
    recentPages: 'Недавние',
    offlineTitle: 'Гейтвей недоступен',
    offlineHint: 'Запустите HTTP-гейтвей и повторите:',
    retry: 'Повторить',
    emptyWikiTitle: 'Вики пуста',
    emptyWikiHint: 'Создайте первую страницу здесь или загрузите документы через инструменты rag-mcp.',
    createFirstPage: 'Создать первую страницу',
    noPagesHint: 'Создайте страницу кнопкой + выше.',
    writeMarkdown: 'Пишите markdown… [[wikilinks]] и #теги',
    casHint: 'CAS: сохранение шлёт if_match_revision с последней загрузки. Конфликт → переоткройте и слейте.',
    backlinks: 'Обратные ссылки',
    incomingWikilinks: 'Входящие [[wikilinks]]',
    noIncoming: 'Нет входящих ссылок',
    properties: 'Свойства',
    kind: 'Тип',
    layer: 'Слой',
    revision: 'Ревизия',
    updated: 'Обновлено',
    outline: 'Содержание',
    history: 'История',
    breadcrumb: 'Навигация',
    palettePlaceholder: 'Перейти к странице или выполнить действие…',
    paletteNoMatches: 'Ничего не найдено',
    paletteNav: '↑↓ навигация',
    paletteOpen: '↵ открыть',
    paletteClose: 'esc закрыть',
    paletteActionNew: 'Новая страница',
    paletteActionGraph: 'Открыть граф',
    paletteActionSearch: 'Открыть поиск',
    searchHeading: 'Поиск',
    searchSubtitle: 'Быстрый переход по каталогу вики + поиск узла графа.',
    searchPlaceholder: 'Запрос…',
    searchNoResults: 'Пока нет результатов',
    graphNodes: '{count} узлов',
    graphEdges: '{count} рёбер',
    graphFull: 'Весь граф',
    graphExpand: 'Развернуть узел',
    graphSeedPlaceholder: 'id / label / document_id узла',
    graphDepth: 'глубина',
    graphMax: 'макс.',
    graphTags: 'теги',
    graphFit: 'Вписать',
    graphFocus: 'Фокус на соседях',
    graphLoading: 'Загрузка графа…',
    graphNoNodes: 'Нет узлов',
    graphOpenAsWiki: 'Открыть в вики',
    graphMinimapHint: 'Обзор (клик — навигация)',
    graphEmptyMinimap: 'пусто',
    layoutForce: 'Силы',
    layoutRadial: 'Радиальная',
    createPrompt: 'Название новой страницы',
    createTitleRequired: 'Нужно название',
    createNoId: 'Страница создана, но document_id не вернулся',
    pageCreated: 'Страница создана',
    saved: 'Сохранено',
    saveConflictToast: 'Конфликт CAS (409): страницу изменили в другом месте. Черновик остался в редакторе.',
    saveConflictTitle: 'Конфликт сохранения: страницу изменил другой писатель (CAS 409).',
    saveConflictRemote: 'Удалённая: r{rev} - "{title}"',
    saveConflictDraft: 'Ваш черновик: "{title}"',
    saveConflictKeep: 'Оставить черновик и сохранить поверх удалённой ревизии',
    saveConflictDiscard: 'Отбросить черновик и загрузить удалённую страницу',
    saveConflictReloadFail: 'Не удалось перезагрузить после конфликта: {msg}',
    saveConflictReloaded: 'Удалённая версия загружена; черновик отброшен',
    saveConflictSavedOver: 'Сохранено поверх удалённой ревизии',
    saveConflictStillFailing: 'Конфликт повторился. Перезагрузите или слейте вручную, затем сохраните снова.',
    confirm: 'Подтвердить',
    keepDraft: 'Оставить черновик',
    discardReload: 'Отбросить и загрузить',
  },
}

export function formatMessage(
  locale: Locale,
  key: MessageKey,
  params?: Record<string, string | number>,
): string {
  let s = messages[locale][key] ?? messages.en[key] ?? key
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.replaceAll(`{${k}}`, String(v))
    }
  }
  return s
}
