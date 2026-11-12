// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { baseName, downloadBlob } from './download'

describe('baseName', () => {
  it('takes the last segment of POSIX and Windows paths', () => {
    expect(baseName('/data/docs/spec.md')).toBe('spec.md')
    expect(baseName('C:\\data\\spec.md')).toBe('spec.md')
    expect(baseName('spec.md')).toBe('spec.md')
  })

  it('falls back when the path has no file name', () => {
    expect(baseName('/data/docs/')).toBe('source')
    expect(baseName('', 'download')).toBe('download')
  })
})

describe('downloadBlob', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    // jsdom does not implement object URLs.
    URL.createObjectURL = vi.fn(() => 'blob:mock')
    URL.revokeObjectURL = vi.fn()
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('clicks a temporary download link and revokes the URL later', () => {
    const clicked: { href: string; download: string; attached: boolean }[] = []
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push({ href: this.href, download: this.download, attached: this.isConnected })
    })
    downloadBlob(new Blob(['x']), 'spec.md')
    expect(clicked).toEqual([{ href: 'blob:mock', download: 'spec.md', attached: true }])
    expect(document.querySelector('a')).toBeNull()
    expect(URL.revokeObjectURL).not.toHaveBeenCalled()
    vi.advanceTimersByTime(60_000)
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock')
  })
})
