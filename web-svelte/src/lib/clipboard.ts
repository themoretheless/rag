/**
 * Copy text to the system clipboard. Resolves to false instead of throwing when the
 * clipboard is unavailable (insecure origin, denied permission, unfocused document),
 * so a click handler can report the failure rather than leave a rejected promise.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    // `navigator.clipboard` is undefined outside secure contexts.
    const clipboard = navigator.clipboard as Clipboard | undefined
    if (!clipboard) return false
    await clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}
