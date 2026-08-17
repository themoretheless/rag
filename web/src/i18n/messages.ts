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
  | 'selectPage'
  | 'selectPageHint'
  | 'linksHint'
  | 'untitled'
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
    selectPage: 'Select a page',
    selectPageHint: 'Pick a note in the sidebar, or press ⌘K to jump.',
    linksHint: 'Blue [[links]] resolve; grey = missing page.',
    untitled: 'Untitled',
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
    selectPage: 'Выберите страницу',
    selectPageHint: 'Выберите заметку в боковой панели или нажмите ⌘K.',
    linksHint: 'Синие [[ссылки]] ведут на страницы; серые - отсутствуют.',
    untitled: 'Без названия',
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
