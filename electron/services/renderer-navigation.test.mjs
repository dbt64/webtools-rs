import test from 'node:test'
import assert from 'node:assert/strict'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { isAllowedRendererNavigation } from './renderer-navigation.ts'

const entry = join('C:\\Program Files\\WebTools\\renderer', 'index.html')
const entryUrl = pathToFileURL(entry).href

test('only the exact packaged renderer entry may navigate', () => {
  assert.equal(isAllowedRendererNavigation(entryUrl, entry), true)
  assert.equal(isAllowedRendererNavigation(`${entryUrl}?request=1#focus`, entry), true)
  assert.equal(isAllowedRendererNavigation(pathToFileURL(join('C:\\Users\\public', 'evil.html')).href, entry), false)
  assert.equal(isAllowedRendererNavigation(pathToFileURL(join('C:\\Program Files\\WebTools\\renderer', 'translation.html')).href, entry), false)
  assert.equal(isAllowedRendererNavigation('https://example.com/', entry), false)
  assert.equal(isAllowedRendererNavigation('javascript:alert(1)', entry), false)
  assert.equal(isAllowedRendererNavigation('not a url', entry), false)
})

test('the development entry is allowed only on its exact origin and path', () => {
  const dev = 'http://localhost:5173/'
  assert.equal(isAllowedRendererNavigation('http://localhost:5173/', entry, dev), true)
  assert.equal(isAllowedRendererNavigation('http://localhost:5173/index.html', entry, dev), false)
  assert.equal(isAllowedRendererNavigation('http://localhost:5173/other.html', entry, dev), false)
  assert.equal(isAllowedRendererNavigation('http://127.0.0.1:5173/', entry, dev), false)
  assert.equal(isAllowedRendererNavigation('https://localhost:5173/', entry, dev), false)

  const translationDev = 'http://localhost:5173/translation.html'
  assert.equal(isAllowedRendererNavigation('http://localhost:5173/translation.html', entry, translationDev), true)
  assert.equal(isAllowedRendererNavigation('http://localhost:5173/', entry, translationDev), false)
})

test('a UNC entry is bound to its own host, not only to the share path', () => {
  const uncEntry = join('\\\\server\\share\\app', 'index.html')
  assert.equal(isAllowedRendererNavigation('file://server/share/app/index.html', uncEntry), true)
  assert.equal(isAllowedRendererNavigation('file://evil/share/app/index.html', uncEntry), false)
  assert.equal(isAllowedRendererNavigation('file://server.evil/share/app/index.html', uncEntry), false)
  assert.equal(isAllowedRendererNavigation('file://user:secret@server/share/app/index.html', uncEntry), false)
})

test('a malformed expected file fails closed instead of throwing', () => {
  assert.equal(isAllowedRendererNavigation(entryUrl, ''), false)
})
