import { IPC_CHANNELS, type IpcResult, type ManagedScheduledTask } from '../../src/shared/ipc.ts'
import { createManagerInvokeRegistry, type IpcSenderContext, type ManagerInvokeRegistry } from './window-security.ts'

export function registerScheduledTasksIpc(ipc: ManagerInvokeRegistry, deps: {
  isManagerMainFrame: (event: IpcSenderContext) => boolean
  list: () => Promise<ManagedScheduledTask[]>
  remove: (pluginId: string, code: string) => Promise<boolean>
}): void {
  // This is a Manager-only administration surface. Plugin SDK calls retain
  // their existing authority-bound identity and cannot enumerate other plugins.
  const manager = createManagerInvokeRegistry(ipc, deps.isManagerMainFrame)
  const failed = (error: unknown) => ({ ok: false as const, error: { code: 'SCHEDULE_UNAVAILABLE',
    message: error instanceof Error ? error.message : '无法连接定时任务服务，请确认 WebTools 托盘仍在运行。' } })
  manager.handle(IPC_CHANNELS.pluginTasksList, async (): Promise<IpcResult<ManagedScheduledTask[]>> => {
    try { return { ok: true, data: await deps.list() } } catch (error) { return failed(error) }
  })
  manager.handle(IPC_CHANNELS.pluginTasksRemove, async (_event, pluginId, code): Promise<IpcResult<boolean>> => {
    if (typeof pluginId !== 'string' || pluginId.length < 3 || pluginId.length > 128
      || typeof code !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(code)) {
      return { ok: false, error: { code: 'INVALID_REQUEST', message: '任务标识无效，未删除任务。' } }
    }
    try { return { ok: true, data: await deps.remove(pluginId, code) } } catch (error) { return failed(error) }
  })
}
