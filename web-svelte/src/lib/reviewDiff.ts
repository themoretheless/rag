/** Exact linear-time replacement diff: unchanged prefix/suffix, replaced middle.
 * Deliberately avoids quadratic LCS on large source-derived wiki pages.
 */
export function reviewDiff(before: string, after: string): { kind: 'same' | 'removed' | 'added'; text: string }[] {
  const a = before.split('\n'), b = after.split('\n')
  let start = 0, end = 0
  while (start < a.length && start < b.length && a[start] === b[start]) start++
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++
  return [
    ...a.slice(0,start).map(text => ({kind:'same' as const,text})),
    ...a.slice(start,a.length-end).map(text => ({kind:'removed' as const,text})),
    ...b.slice(start,b.length-end).map(text => ({kind:'added' as const,text})),
    ...a.slice(a.length-end).map(text => ({kind:'same' as const,text})),
  ]
}
