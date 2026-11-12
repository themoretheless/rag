export type ToastKind = 'info' | 'error' | 'ok'

export interface Toast {
  id: number
  text: string
  kind: ToastKind
}

/** How long a toast stays on screen. */
export const TOAST_TTL_MS = 4200

/** Transient notifications shown by the shell's toast host. */
export class Toasts {
  items = $state<Toast[]>([])

  private seq = 0

  /** Show a toast; it removes itself after `TOAST_TTL_MS`. */
  push(text: string, kind: ToastKind = 'info') {
    const id = ++this.seq
    this.items.push({ id, text, kind })
    setTimeout(() => {
      this.items = this.items.filter((toast) => toast.id !== id)
    }, TOAST_TTL_MS)
  }
}
