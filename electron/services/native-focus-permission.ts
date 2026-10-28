import { spawn } from 'node:child_process'

export async function grantNativeHostForegroundPermission(
  executable: string | undefined,
  processId: number,
  start: typeof spawn = spawn,
): Promise<boolean> {
  if (!executable || !Number.isInteger(processId) || processId <= 0) return false

  return await new Promise(resolve => {
    const helper = start(executable, ['--allow-foreground', String(processId)], {
      windowsHide: true,
      stdio: 'ignore',
    })
    helper.once('error', () => resolve(false))
    helper.once('exit', code => resolve(code === 0))
  })
}
