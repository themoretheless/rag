/** Notion/Obsidian-ish markdown → HTML for wiki article view. */

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

function linkResolved(key: string, titles: Set<string>, slugs: Set<string>): boolean {
  const k = key.trim()
  if (titles.has(k) || slugs.has(k)) return true
  const lower = k.toLowerCase()
  for (const t of titles) if (t.toLowerCase() === lower) return true
  for (const s of slugs) if (s.toLowerCase() === lower) return true
  return false
}

function inlineFormat(text: string, titles: Set<string>, slugs: Set<string>): string {
  let s = escapeHtml(text)
  // wikilinks [[target|label]] or [[target]]
  s = s.replace(/\[\[([^\]]+)\]\]/g, (_m, inner: string) => {
    const parts = inner.split('|')
    const key = (parts[0] ?? '').trim()
    const label = (parts[1] ?? key).trim()
    const ok = linkResolved(key, titles, slugs)
    const cls = ok ? 'wikilink' : 'wikilink missing'
    return `<a href="#" class="${cls}" data-wikilink="${escapeHtml(key)}">${escapeHtml(label)}</a>`
  })
  // `code`
  s = s.replace(/`([^`]+)`/g, '<code>$1</code>')
  // **bold**
  s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
  // *italic*
  s = s.replace(/(^|[^*])\*([^*]+)\*(?!\*)/g, '$1<em>$2</em>')
  // #tags (not headings)
  s = s.replace(/(^|\s)#([A-Za-z0-9_/-]+)/g, '$1<span class="tag">#$2</span>')
  return s
}

/**
 * Split a GFM pipe row into cell strings (outer pipes optional).
 * Pipes inside `[[wikilinks]]` or `code` are not cell separators.
 */
function splitPipeCells(line: string): string[] {
  let s = line.trim()
  if (s.startsWith('|')) s = s.slice(1)
  if (s.endsWith('|')) s = s.slice(0, -1)

  const cells: string[] = []
  let cur = ''
  let i = 0
  while (i < s.length) {
    // wikilink: [[...]] (may contain |)
    if (s[i] === '[' && s[i + 1] === '[') {
      const end = s.indexOf(']]', i + 2)
      if (end !== -1) {
        cur += s.slice(i, end + 2)
        i = end + 2
        continue
      }
    }
    // inline code: `...`
    if (s[i] === '`') {
      const end = s.indexOf('`', i + 1)
      if (end !== -1) {
        cur += s.slice(i, end + 1)
        i = end + 1
        continue
      }
    }
    if (s[i] === '|') {
      cells.push(cur.trim())
      cur = ''
      i++
      continue
    }
    cur += s[i]
    i++
  }
  cells.push(cur.trim())
  return cells
}

/** GFM separator: | --- | :---: | ---: | (colons set alignment). */
function isSeparatorRow(line: string): boolean {
  const t = line.trim()
  if (!t || !t.includes('-')) return false
  // Only pipes, dashes, colons, spaces
  if (!/^[\s|:-]+$/.test(t)) return false
  const cells = splitPipeCells(t)
  if (cells.length === 0) return false
  return cells.every((c) => /^:?-{1,}:?$/.test(c))
}

/** A pipe row with at least one cell separator (not a bare paragraph). */
function isPipeRow(line: string): boolean {
  const t = line.trim()
  if (!t.includes('|')) return false
  // Avoid treating bare | or non-tabular lines as rows
  const cells = splitPipeCells(t)
  return cells.length >= 1 && (t.match(/\|/g) ?? []).length >= 1
}

type CellAlign = 'left' | 'center' | 'right' | null

function parseAligns(sepCells: string[]): CellAlign[] {
  return sepCells.map((c) => {
    const left = c.startsWith(':')
    const right = c.endsWith(':')
    if (left && right) return 'center'
    if (right) return 'right'
    if (left) return 'left'
    return null
  })
}

function alignAttr(a: CellAlign | undefined): string {
  if (!a) return ''
  return ` align="${a}"`
}

export interface OutlineHeading {
  id: string
  level: 1 | 2 | 3
  text: string
}

/** Plain heading text for outline labels (strips common inline markdown). */
export function plainHeadingText(raw: string): string {
  return raw
    .replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_m, t: string, l?: string) =>
      (l ?? t).trim(),
    )
    .replace(/`([^`]+)`/g, '$1')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/(^|[^*])\*([^*]+)\*(?!\*)/g, '$1$2')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .trim()
}

export function slugifyHeading(text: string): string {
  const base = text
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .replace(/[^a-z0-9\s-]/g, '')
    .trim()
    .replace(/\s+/g, '-')
    .replace(/-+/g, '-')
  return base || 'section'
}

/**
 * Collect # / ## / ### headings from wiki markdown (skips fenced code).
 * Ids are unique per document and match renderWikiHtml anchors.
 */
export function extractHeadings(content: string): OutlineHeading[] {
  const lines = content.replace(/\r\n/g, '\n').split('\n')
  const out: OutlineHeading[] = []
  const used = new Map<string, number>()
  let inCode = false

  for (const line of lines) {
    if (line.trimStart().startsWith('```')) {
      inCode = !inCode
      continue
    }
    if (inCode) continue
    const m = line.trimEnd().match(/^(#{1,3})\s+(.+)$/)
    if (!m) continue
    const level = m[1]!.length as 1 | 2 | 3
    const text = plainHeadingText(m[2] ?? '')
    if (!text) continue
    let id = slugifyHeading(text)
    const n = used.get(id) ?? 0
    used.set(id, n + 1)
    if (n > 0) id = `${id}-${n}`
    out.push({ id, level, text })
  }
  return out
}

function nextHeadingId(raw: string, usedIds: Map<string, number>): string {
  const text = plainHeadingText(raw)
  let id = slugifyHeading(text)
  const n = usedIds.get(id) ?? 0
  usedIds.set(id, n + 1)
  if (n > 0) id = `${id}-${n}`
  return id
}

function renderTable(
  header: string[],
  aligns: CellAlign[],
  body: string[][],
  titles: Set<string>,
  slugs: Set<string>,
): string {
  const th = header
    .map((cell, i) => {
      const a = aligns[i] ?? null
      return `<th${alignAttr(a)}>${inlineFormat(cell, titles, slugs)}</th>`
    })
    .join('')
  const rows = body
    .map((row) => {
      // Pad/truncate to header width for stable columns
      const cells: string[] = []
      for (let i = 0; i < header.length; i++) {
        cells.push(row[i] ?? '')
      }
      // Extra cells beyond header still render
      for (let i = header.length; i < row.length; i++) {
        cells.push(row[i] ?? '')
      }
      const tds = cells
        .map((cell, i) => {
          const a = aligns[i] ?? null
          return `<td${alignAttr(a)}>${inlineFormat(cell, titles, slugs)}</td>`
        })
        .join('')
      return `<tr>${tds}</tr>`
    })
    .join('')
  return `<table><thead><tr>${th}</tr></thead><tbody>${rows}</tbody></table>`
}

export function renderWikiHtml(
  content: string,
  titles: Set<string>,
  slugs: Set<string>,
): string {
  const lines = content.replace(/\r\n/g, '\n').split('\n')
  const out: string[] = []
  let inCode = false
  let codeBuf: string[] = []
  let i = 0
  const usedIds = new Map<string, number>()

  while (i < lines.length) {
    const line = lines[i] ?? ''

    if (line.trimStart().startsWith('```')) {
      if (inCode) {
        out.push(`<pre><code>${escapeHtml(codeBuf.join('\n'))}</code></pre>`)
        codeBuf = []
        inCode = false
      } else {
        inCode = true
      }
      i++
      continue
    }
    if (inCode) {
      codeBuf.push(line)
      i++
      continue
    }

    // GFM pipe table: header + separator (+ body rows)
    if (
      i + 1 < lines.length &&
      isPipeRow(line) &&
      !isSeparatorRow(line) &&
      isSeparatorRow(lines[i + 1] ?? '')
    ) {
      const header = splitPipeCells(line)
      const aligns = parseAligns(splitPipeCells(lines[i + 1] ?? ''))
      i += 2
      const body: string[][] = []
      while (i < lines.length) {
        const rowLine = lines[i] ?? ''
        if (!rowLine.trim()) break
        if (!isPipeRow(rowLine) || isSeparatorRow(rowLine)) break
        body.push(splitPipeCells(rowLine))
        i++
      }
      out.push(renderTable(header, aligns, body, titles, slugs))
      continue
    }

    const t = line.trimEnd()
    if (!t.trim()) {
      out.push('')
      i++
      continue
    }
    if (t.startsWith('### ')) {
      const raw = t.slice(4)
      const id = nextHeadingId(raw, usedIds)
      out.push(
        `<h3 id="${escapeHtml(id)}">${inlineFormat(raw, titles, slugs)}</h3>`,
      )
      i++
      continue
    }
    if (t.startsWith('## ')) {
      const raw = t.slice(3)
      const id = nextHeadingId(raw, usedIds)
      out.push(
        `<h2 id="${escapeHtml(id)}">${inlineFormat(raw, titles, slugs)}</h2>`,
      )
      i++
      continue
    }
    if (t.startsWith('# ')) {
      const raw = t.slice(2)
      const id = nextHeadingId(raw, usedIds)
      out.push(
        `<h1 id="${escapeHtml(id)}">${inlineFormat(raw, titles, slugs)}</h1>`,
      )
      i++
      continue
    }
    if (t.startsWith('> ')) {
      out.push(`<blockquote>${inlineFormat(t.slice(2), titles, slugs)}</blockquote>`)
      i++
      continue
    }
    if (t.startsWith('- ') || t.startsWith('* ')) {
      out.push(`<li>${inlineFormat(t.slice(2), titles, slugs)}</li>`)
      i++
      continue
    }
    const num = t.match(/^(\d+)\.\s+(.*)$/)
    if (num) {
      out.push(`<li value="${num[1]}">${inlineFormat(num[2] ?? '', titles, slugs)}</li>`)
      i++
      continue
    }
    out.push(`<p>${inlineFormat(t, titles, slugs)}</p>`)
    i++
  }
  if (inCode && codeBuf.length) {
    out.push(`<pre><code>${escapeHtml(codeBuf.join('\n'))}</code></pre>`)
  }

  // wrap consecutive li
  const joined = out.join('\n')
  return joined
    .replace(/(?:<li[\s\S]*?<\/li>\n?)+/g, (block) => {
      if (block.includes('value=')) return `<ol>${block}</ol>`
      return `<ul>${block}</ul>`
    })
}
