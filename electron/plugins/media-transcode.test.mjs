import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, writeFile, readFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { transcodeMedia } from './media-transcode.ts'
import { validateCapabilityRequest } from '../../plugin-sdk/declarative-v1/web/capability-methods.mjs'

test('real FFmpeg converts WAV to MP3 and back; failed and cancelled jobs preserve destination', async t => {
  const root = await mkdtemp(join(tmpdir(), 'webtools-media-')); t.after(() => rm(root, { recursive: true, force: true }))
  const wav = Buffer.alloc(44 + 16000 * 2)
  wav.write('RIFF'); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8); wav.writeUInt32LE(16, 16)
  wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22); wav.writeUInt32LE(16000, 24); wav.writeUInt32LE(32000, 28)
  wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34); wav.write('data', 36); wav.writeUInt32LE(32000, 40)
  const source = join(root, '测试.wav'), mp3 = join(root, '结果.mp3'), decoded = join(root, 'decoded.wav')
  await writeFile(source, wav)
  const signal = new AbortController().signal
  await transcodeMedia({ inputPath: source, format: 'mp3' }, mp3, signal)
  assert.ok((await readFile(mp3)).length > 100)
  await transcodeMedia({ inputPath: mp3, format: 'wav' }, decoded, signal)
  assert.equal((await readFile(decoded)).subarray(0, 4).toString(), 'RIFF')
  await assert.rejects(transcodeMedia({ inputPath: source, format: 'wav' }, source, signal), e => e.code === 'INVALID_REQUEST')
  await writeFile(mp3, 'existing destination')
  await assert.rejects(transcodeMedia({ inputPath: join(root, 'missing'), format: 'mp3' }, mp3, signal))
  const cancelled = AbortSignal.abort()
  await assert.rejects(transcodeMedia({ inputPath: source, format: 'mp3' }, mp3, cancelled), e => e.code === 'CANCELLED')
  assert.equal(await readFile(mp3, 'utf8'), 'existing destination')
  assert.ok(!(await readdir(root)).some(file => file.startsWith('.webtools-')))
})

test('workflow protocol rejects executable URLs, extra code and excessive image sizes', () => {
  for (const url of ['file:///C:/Windows/win.ini', 'javascript:alert(1)', 'https://user:password@example.com']) {
    assert.throws(() => validateCapabilityRequest('system.browserOpen', { url }))
  }
  assert.throws(() => validateCapabilityRequest('system.browserStep', { id: 'x', action: 'eval', value: 'process.exit()' }))
  assert.throws(() => validateCapabilityRequest('system.imageTransform', { dataUrl: 'data:image/png;base64,AAAA', width: 99999, format: 'png', quality: 85 }))
})
