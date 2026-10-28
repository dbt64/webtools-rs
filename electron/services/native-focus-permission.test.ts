import test from 'node:test'
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { grantNativeHostForegroundPermission } from './native-focus-permission.ts'

test('foreground handoff starts the native helper with the host PID and waits for its result', async () => {
  const child = new EventEmitter()
  let launch: unknown[] | undefined
  const start = ((...args: unknown[]) => { launch = args; return child }) as never

  const result = grantNativeHostForegroundPermission('WebTools.NativeHost.Rust.exe', 25800, start)
  assert.equal(launch?.[0], 'WebTools.NativeHost.Rust.exe')
  assert.deepEqual(launch?.[1], ['--allow-foreground', '25800'])
  assert.deepEqual(launch?.[2], { windowsHide: true, stdio: 'ignore' })

  child.emit('exit', 0)
  assert.equal(await result, true)
})

test('foreground handoff reports launch failure without claiming permission', async () => {
  const child = new EventEmitter()
  const start = (() => child) as never
  const result = grantNativeHostForegroundPermission('WebTools.NativeHost.Rust.exe', 25800, start)
  child.emit('exit', 1)
  assert.equal(await result, false)
})
