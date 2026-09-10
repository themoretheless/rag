// @vitest-environment jsdom

import { describe, expect, it } from 'vitest'
import mermaid from 'mermaid'
import { renderWikiHtml } from './markdown'

const emptySet = new Set<string>()

describe('Mermaid wiki fences', () => {
  it('emits an escaped Mermaid render target instead of a code block', () => {
    const html = renderWikiHtml(
      ['```mermaid', 'flowchart LR', '  A[Input] --> B[Output]', '```'].join('\n'),
      emptySet,
      emptySet,
    )

    expect(html).toContain('class="mermaid-diagram"')
    expect(html).toContain('data-mermaid-source')
    expect(html).toContain('A[Input] --&gt; B[Output]')
    expect(html).not.toContain('<pre>')
  })

  it('recognizes the Mermaid language case-insensitively', () => {
    const html = renderWikiHtml('```MERMAID\nsequenceDiagram\nA->>B: Ping\n```', emptySet, emptySet)

    expect(html).toContain('class="mermaid-diagram"')
  })

  it('keeps non-Mermaid fences as syntax-labelled code', () => {
    const html = renderWikiHtml('```rust\nfn main() {}\n```', emptySet, emptySet)

    expect(html).toBe('<pre><code class="language-rust">fn main() {}</code></pre>')
  })

  it('never injects HTML from a diagram source', () => {
    const html = renderWikiHtml(
      '```mermaid\nflowchart LR\nA[<img src=x onerror=alert(1)>] --> B\n```',
      emptySet,
      emptySet,
    )

    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
    expect(html).not.toContain('<img')
  })
})

describe('Mermaid syntax used by the wiki', () => {
  const diagrams = {
    flowchart: 'flowchart LR\nClient --> API\nAPI --> DB[(MySQL)]',
    sequence: 'sequenceDiagram\nClient->>API: Redeem\nAPI-->>Client: Result',
    state: 'stateDiagram-v2\n[*] --> Pending\nPending --> Rewarded',
    er: 'erDiagram\nREFERRER ||--o{ REFERRAL : invites',
  }

  for (const [name, source] of Object.entries(diagrams)) {
    it(`parses the ${name} diagram`, async () => {
      await expect(mermaid.parse(source)).resolves.toBeTruthy()
    })
  }
})
