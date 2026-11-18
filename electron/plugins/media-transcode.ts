import { spawn } from 'node:child_process'
import { rename, rm } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { randomUUID } from 'node:crypto'
import ffmpeg from 'ffmpeg-static'
import { BridgeWireError } from '../../plugin-sdk/declarative-v1/web/protocol.mjs'
import type { MediaTranscode } from '../../plugin-sdk/declarative-v1/web/capability-methods.mjs'

/** Write a sibling temporary file; failed/cancelled jobs never replace an existing destination. */
export async function transcodeMedia(input: MediaTranscode, destination: string, signal: AbortSignal): Promise<{ path: string }> {
  if (signal.aborted) throw new BridgeWireError('CANCELLED')
  if (!ffmpeg) throw new BridgeWireError('NOT_AVAILABLE')
  if (resolve(input.inputPath).toLowerCase() === resolve(destination).toLowerCase()) throw new BridgeWireError('INVALID_REQUEST')
  const executablePath = ffmpeg
  const temporary = join(dirname(destination), `.webtools-${randomUUID()}.${input.format}`)
  const codecs: Record<MediaTranscode['format'], string[]> = {
    wav: ['-vn', '-c:a', 'pcm_s16le'], mp3: ['-vn', '-c:a', 'libmp3lame', '-q:a', '2'],
    mp4: ['-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-movflags', '+faststart'],
    webm: ['-c:v', 'libvpx-vp9', '-deadline', 'realtime', '-cpu-used', '6', '-c:a', 'libopus'],
  }
  try {
    await new Promise<void>((accept, reject) => {
      // electron-builder unpacks the executable; never execute a shell command from plugin text.
      const executable = executablePath.replace(/app\.asar([\\/])/u, 'app.asar.unpacked$1')
      const child = spawn(executable, ['-nostdin', '-hide_banner', '-loglevel', 'error', '-y', '-i', input.inputPath, ...codecs[input.format], temporary], { windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] })
      let detail = ''; let timedOut = false
      child.stderr.on('data', data => { detail = (detail + String(data)).slice(-4096) })
      const abort = () => child.kill()
      const timer = setTimeout(() => { timedOut = true; child.kill() }, 120000)
      signal.addEventListener('abort', abort, { once: true })
      if (signal.aborted) child.kill()
      const cleanup = () => { clearTimeout(timer); signal.removeEventListener('abort', abort) }
      child.once('error', error => { cleanup(); reject(error) })
      child.once('close', code => {
        cleanup()
        if (signal.aborted) reject(new BridgeWireError('CANCELLED'))
        else if (timedOut) reject(new BridgeWireError('TIMEOUT'))
        else if (code !== 0) reject(new Error(detail || 'Media conversion failed'))
        else accept()
      })
    })
    if (signal.aborted) throw new BridgeWireError('CANCELLED')
    await rename(temporary, destination)
    return { path: destination }
  } finally { await rm(temporary, { force: true }) }
}
