import { expect, it } from 'vitest'
import { reviewDiff } from './reviewDiff'
it('reconstructs both versions including empty lines and insertion/deletion', () => {
  for (const [before, after] of [['a\nb\nc','a\nx\nc'],['','x'],['x',''],['a\n','a\n\nb'],['same','same']]) {
    const diff = reviewDiff(before, after)
    expect(diff.filter(line => line.kind !== 'added').map(line => line.text).join('\n')).toBe(before)
    expect(diff.filter(line => line.kind !== 'removed').map(line => line.text).join('\n')).toBe(after)
  }
})
it('keeps common context and marks the replaced middle', () => {
  expect(reviewDiff('a\nb\nc','a\nx\nc')).toEqual([
    {kind:'same',text:'a'},{kind:'removed',text:'b'},{kind:'added',text:'x'},{kind:'same',text:'c'},
  ])
})
