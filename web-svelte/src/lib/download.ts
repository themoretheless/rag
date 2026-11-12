/** How long a blob URL stays alive after the click that starts the download. */
const REVOKE_AFTER_MS = 60_000

/** File name of a server-side path (`/a/b/c.md`, `C:\a\c.md` → `c.md`); `fallback` when empty. */
export function baseName(path: string, fallback = 'source'): string {
  return path.split(/[\\/]/).pop() || fallback
}

/**
 * Save a blob through a temporary `<a download>`. Needed for downloads that require
 * an Authorization header, which a plain link cannot send.
 */
export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  document.body.appendChild(link)
  link.click()
  link.remove()
  // The browser reads the URL asynchronously; revoke it once the download has surely started.
  setTimeout(() => {
    URL.revokeObjectURL(url)
  }, REVOKE_AFTER_MS)
}
