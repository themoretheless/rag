/** Link completion only inside a single unfinished wikilink before the caret. */
export function linkAtCaret(text: string, caret: number) {
  const start = text.lastIndexOf('[[', caret)
  if (start < 0 || start + 2 > caret) return null
  const query = text.slice(start + 2, caret)
  if (/[\]\[\n|]/.test(query)) return null
  return { start, end: caret, query }
}
export function insertWikiLink(text: string, start: number, end: number, slug: string, title: string) {
  const safeTitle = title.replace(/[\[\]|\r\n]/g, ' ')
  const link = `[[${slug}|${safeTitle}]]`
  const tail = text.slice(end).startsWith(']]') ? end + 2 : end
  return { text: text.slice(0,start) + link + text.slice(tail), caret: start + link.length }
}
export const wikiTemplates = {
  decision: '# Контекст\n\n## Решение\n\n## Причины\n\n## Рассмотренные варианты\n\n## Последствия\n\n## Источники\n',
  guide: '# Назначение\n\n## Перед началом\n\n## Порядок действий\n\n1. \n\n## Проверка результата\n\n## Если что-то пошло не так\n\n## Источники\n',
  service: '# Назначение сервиса\n\n## Границы ответственности\n\n## Компоненты и зависимости\n\n## Контракты\n\n## Эксплуатация\n\n## Источники\n',
  research: '# Вопрос\n\n## Подтверждённые факты\n\n## Гипотезы\n\n## Открытые вопросы\n\n## Выводы\n\n## Источники\n',
}
