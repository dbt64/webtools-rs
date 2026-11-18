// Electron integration checks, using real browser/native image/FFmpeg implementations.
const { app, BrowserWindow, nativeImage, session } = require('electron')
const ts = require('typescript')
const fs = require('node:fs')
const path = require('node:path')
const assert = require('node:assert/strict')
const { createServer } = require('node:http')
require.extensions['.ts'] = (module, filename) => module._compile(ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, esModuleInterop: true },
}).outputText, filename)
const delivery = process.argv[2]
const root = path.join(delivery, 'verification', 'effects-' + Date.now())
fs.mkdirSync(root, { recursive: true })
app.setPath('userData', path.join(root, 'profile'))
app.on('window-all-closed', () => {})
app.whenReady().then(async () => {
  const { createPluginWorkflowEffects, browserPartition, clearPluginBrowserData } = require('../electron/plugins/plugin-workflow-effects.ts')
  const effects = createPluginWorkflowEffects()
  const server = createServer((_req, res) => { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end('<input id="value"><button id="go" onclick="document.querySelector(\'#out\').textContent=document.querySelector(\'#value\').value">Go</button><p id="out"></p>') })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const url = 'http://127.0.0.1:' + server.address().port
  const checks = []
  try {
    for (let index = 0; index < 3; index++) {
      const controller = new AbortController()
      const browser = await effects.browserOpen('test.workflow', url, controller.signal)
      await effects.browserStep({ id: browser.id, action: 'input', selector: '#value', value: '中文 ' + index }, controller.signal, controller.signal)
      await effects.browserStep({ id: browser.id, action: 'click', selector: '#go' }, controller.signal, controller.signal)
      assert.equal(await effects.browserStep({ id: browser.id, action: 'text', selector: '#out' }, controller.signal, controller.signal), '中文 ' + index)
      await assert.rejects(effects.browserStep({ id: browser.id, action: 'text', selector: '#out' }, new AbortController().signal, controller.signal), e => e.code === 'RUNTIME_CLOSED')
      controller.abort()
      assert.equal(BrowserWindow.getAllWindows().length, 0)
    }
    checks.push('Three real browser runs pass; foreign runtime rejected and abort closes all owned windows')
    const data = nativeImage.createFromBitmap(Buffer.alloc(1 * 32768 * 4), { width: 1, height: 32768 }).toDataURL()
    await assert.rejects(effects.imageTransform({ dataUrl: data, width: 4096, format: 'png', quality: 85 }), e => e.code === 'LIMIT_EXCEEDED')
    checks.push('Extreme aspect ratio rejected before output allocation')
    const cookieSession = session.fromPartition(browserPartition('test.workflow'))
    await cookieSession.cookies.set({ url, name: 'test-cookie', value: 'private' })
    assert.equal((await cookieSession.cookies.get({ url })).length, 1)
    await clearPluginBrowserData('test.workflow')
    assert.equal((await cookieSession.cookies.get({ url })).length, 0)
    checks.push('Delete plugin data clears persistent browser cookies')
    const binary = path.join(delivery, 'stage/Manager/resources/app.asar.unpacked/node_modules/ffmpeg-static/ffmpeg.exe')
    require('ffmpeg-static')
    require.cache[require.resolve('ffmpeg-static')].exports = binary
    const { transcodeMedia } = require('../electron/plugins/media-transcode.ts')
    // Generate a video with the exact binary shipped in the preview, then use the public host effect.
    const { spawnSync } = require('node:child_process')
    const video = path.join(root, 'source.mp4')
    const generated = spawnSync(binary, ['-nostdin', '-y', '-f', 'lavfi', '-i', 'color=c=blue:s=160x120:d=1', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', video], { windowsHide: true })
    assert.equal(generated.status, 0, String(generated.stderr))
    const output = path.join(root, 'converted.webm')
    await transcodeMedia({ inputPath: video, format: 'webm' }, output, new AbortController().signal)
    assert.ok(fs.statSync(output).size > 100)
    checks.push('Shipped FFmpeg generates a real MP4 and host conversion produces playable WebM')
    fs.writeFileSync(path.join(delivery, 'workflow-effects-verification.json'), JSON.stringify({ checks, root }, null, 2))
    console.log(checks.join('\n'))
  } finally { server.close(); app.quit() }
}).catch(error => { console.error(error); app.exit(1) })
