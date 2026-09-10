import { expect, it } from 'vitest'
import { linkAtCaret, insertWikiLink } from './wikiEditing'
it('completes only unfinished links at the caret', () => {
  expect(linkAtCaret('Text [[ref',10)).toEqual({start:5,end:10,query:'ref'})
  expect(linkAtCaret('[[closed]] text',15)).toBeNull()
  expect(linkAtCaret('[[a\nb',6)).toBeNull()
  expect(linkAtCaret('[[a|label',9)).toBeNull()
})
it('preserves the following text and does not duplicate closing brackets', () => {
  expect(insertWikiLink('Text [[ab]] tail',5,9,'slug','Title')).toEqual({text:'Text [[slug|Title]] tail',caret:19})
  expect(insertWikiLink('[[ab tail',0,4,'slug','bad|title').text).toBe('[[slug|bad title]] tail')
})
