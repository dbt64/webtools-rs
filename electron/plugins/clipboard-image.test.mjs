import test from 'node:test'
import assert from 'node:assert/strict'
import { readClipboardImage } from './clipboard-image.ts'

test('reads an image file copied in Explorer when no bitmap is present', async () => {
  const result = await readClipboardImage({ readItems: async () => [], readFiles: async () => ['D:\\photo.JPG'], imageFromPath: path => path.endsWith('.JPG') ? 'data:image/png;base64,cG5n' : null })
  assert.equal(result, 'data:image/png;base64,cG5n')
})

test('prefers clipboard image pixels and ignores non-image files', async () => {
  let pathsRead = false
  const bitmap = { types: ['image/png'], getType: async () => new Blob(['png']) }
  assert.equal(await readClipboardImage({ readItems: async () => [bitmap], readFiles: async () => { pathsRead = true; return [] }, imageFromPath: () => null }), 'data:image/png;base64,cG5n')
  assert.equal(pathsRead, false)
  assert.equal(await readClipboardImage({ readItems: async () => [], readFiles: async () => ['D:\\document.txt'], imageFromPath: () => { throw new Error('not an image') } }), null)
})
