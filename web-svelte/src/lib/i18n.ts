/** UI label keys for shell + wiki (en/ru). Message catalog is locale-agnostic. */
export type Locale = 'en' | 'ru'

export type MessageKey =
  | 'wiki'
  | 'graph'
  | 'search'
  | 'theme'
  | 'offline'
  | 'healthStats'
  | 'pages'
  | 'favorites'
  | 'pinFavorite'
  | 'unpinFavorite'
  | 'refresh'
  | 'newPage'
  | 'loading'
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
  | 'graphNodes'
  | 'graphEdges'
  | 'graphFull'
  | 'graphExpand'
  | 'graphDepth'
  | 'graphMax'
  | 'graphFit'
  | 'graphFocus'
  | 'graphLoading'
  | 'graphNoNodes'
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
  | 'saveConflictReloaded'
  | 'saveConflictSavedOver'
  | 'saveConflictStillFailing'
  | 'confirm'
  | 'apiInvalidPath'
  | 'apiUnauthorized'
  | 'apiForbidden'
  | 'apiHttpError'
  | 'apiInvalidJson'
  | 'apiEmptyBody'
  | 'tokenInvalidChars'
  | 'tokenStorageDenied'
  | 'graphProject'
  | 'graphAllProjects'
  | 'graphFocusToggle'
  | 'graphTagsToggle'
  | 'graphZoomIn'
  | 'graphZoomOut'
  | 'graphLabelDensity'
  | 'graphLabelsHubs'
  | 'graphLabelsAll'
  | 'graphLabelsNone'
  | 'graphSeedLabel'
  | 'graphSeedHint'

type Messages = Record<MessageKey, string>

export const messages: Record<Locale, Messages> = {
  en: {
    wiki: 'Wiki',
    graph: 'Graph',
    search: 'Search',
    theme: 'Theme',
    offline: 'offline',
    healthStats: '{docs} docs · {nodes} nodes',
    pages: 'Pages',
    favorites: 'Favorites',
    pinFavorite: 'Pin to favorites',
    unpinFavorite: 'Unpin from favorites',
    refresh: 'Refresh',
    newPage: 'New page',
    loading: 'Loading…',
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
    graphNodes: '{count} nodes',
    graphEdges: '{count} edges',
    graphFull: 'Full graph',
    graphExpand: 'Expand seed',
    graphDepth: 'depth',
    graphMax: 'max',
    graphFit: 'Fit view',
    graphFocus: 'Focus neighbors',
    graphLoading: 'Loading graph…',
    graphNoNodes: 'No nodes',
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
    saveConflictReloaded: 'Reloaded remote page; draft discarded',
    saveConflictSavedOver: 'Saved over remote revision',
    saveConflictStillFailing: 'Still conflicting after retry. Reload remote or merge manually, then save again.',
    confirm: 'Confirm',
    apiInvalidPath: 'Gateway requests require an absolute API path.',
    apiUnauthorized: 'HTTP 401: Provide a valid token in the “Access” section.',
    apiForbidden: 'HTTP 403: The token lacks permission for this action.',
    apiHttpError: 'HTTP {status}: {detail}',
    apiInvalidJson: 'HTTP {status}: response is not valid JSON',
    apiEmptyBody: 'HTTP {status}: expected a JSON response but the body was empty',
    tokenInvalidChars: 'The token may contain only valid Bearer token characters.',
    tokenStorageDenied: 'The browser refused to store the token in this tab’s session.',
    graphProject: 'Project',
    graphAllProjects: 'All projects',
    graphFocusToggle: '◎ Focus',
    graphTagsToggle: '◇ Tags',
    graphZoomIn: 'Zoom in',
    graphZoomOut: 'Zoom out',
    graphLabelDensity: 'Label density',
    graphLabelsHubs: 'Labels: hubs',
    graphLabelsAll: 'Labels: all',
    graphLabelsNone: 'Labels: none',
    graphSeedLabel: 'Seed node',
    graphSeedHint: 'seed · wiki://…',
  },
  ru: {
    wiki: 'Вики',
    graph: 'Граф',
    search: 'Поиск',
    theme: 'Тема',
    offline: 'офлайн',
    healthStats: '{docs} док. · {nodes} узлов',
    pages: 'Страницы',
    favorites: 'Избранное',
    pinFavorite: 'В избранное',
    unpinFavorite: 'Убрать из избранного',
    refresh: 'Обновить',
    newPage: 'Новая страница',
    loading: 'Загрузка…',
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
    graphNodes: '{count} узлов',
    graphEdges: '{count} рёбер',
    graphFull: 'Весь граф',
    graphExpand: 'Развернуть узел',
    graphDepth: 'глубина',
    graphMax: 'макс',
    graphFit: 'Вписать',
    graphFocus: 'Фокус на соседях',
    graphLoading: 'Загрузка графа…',
    graphNoNodes: 'Нет узлов',
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
    saveConflictReloaded: 'Удалённая версия загружена; черновик отброшен',
    saveConflictSavedOver: 'Сохранено поверх удалённой ревизии',
    saveConflictStillFailing: 'Конфликт повторился. Перезагрузите или слейте вручную, затем сохраните снова.',
    confirm: 'Подтвердить',
    apiInvalidPath: 'Запросы к шлюзу требуют абсолютного пути API.',
    apiUnauthorized: 'HTTP 401: Укажите действующий токен в разделе «Доступ».',
    apiForbidden: 'HTTP 403: У токена нет прав для этого действия.',
    apiHttpError: 'HTTP {status}: {detail}',
    apiInvalidJson: 'HTTP {status}: ответ не является корректным JSON',
    apiEmptyBody: 'HTTP {status}: ожидался JSON-ответ, но тело пустое',
    tokenInvalidChars: 'Токен должен содержать только допустимые символы Bearer token.',
    tokenStorageDenied: 'Браузер запретил хранение токена в сессии этой вкладки.',
    graphProject: 'Проект',
    graphAllProjects: 'Все проекты',
    graphFocusToggle: '◎ Фокус',
    graphTagsToggle: '◇ Теги',
    graphZoomIn: 'Приблизить',
    graphZoomOut: 'Отдалить',
    graphLabelDensity: 'Плотность подписей',
    graphLabelsHubs: 'Подписи: hubs',
    graphLabelsAll: 'Подписи: все',
    graphLabelsNone: 'Подписи: нет',
    graphSeedLabel: 'Исходный узел',
    graphSeedHint: 'seed · wiki://…',
  },
}

export function formatMessage(
  locale: Locale,
  key: MessageKey,
  params?: Record<string, string | number>,
): string {
  let s: string = messages[locale][key]
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.replaceAll(`{${k}}`, String(v))
    }
  }
  return s
}

let activeLocale: Locale = 'ru'

/** Locale used by non-component modules (API client); kept in sync by the UI store. */
export function setActiveLocale(locale: Locale): void { activeLocale = locale }

/** Translate with the active locale (for modules outside the Svelte UI store). */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  return formatMessage(activeLocale, key, params)
}
