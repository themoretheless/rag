/** UI label keys for shell + wiki (en/ru). */
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
