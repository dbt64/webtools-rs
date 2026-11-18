import { IPC_CHANNELS, type IpcResult } from '../../src/shared/ipc.ts'
import { parseMcpGatewayStatus, type McpGatewayStatus } from '../../src/shared/plugin-mcp-contracts.ts'
import type { McpAgentConfig } from '../plugins/mcp-agent-config.ts'
import { createManagerInvokeRegistry, type IpcSenderContext, type ManagerInvokeRegistry } from './window-security.ts'
interface Tool { pluginId: string; featureId: string; title: string; description: string }
interface Dependencies {
  isManagerMainFrame(event: IpcSenderContext): boolean
  request(type: string, payload: unknown): Promise<unknown>
  listTools(): Tool[]
  agents: Pick<McpAgentConfig, 'list' | 'install'>
}
export function registerPluginMcpIpc(ipc: ManagerInvokeRegistry, deps: Dependencies): void {
  const manager = createManagerInvokeRegistry(ipc, deps.isManagerMainFrame)
  const protect = async <T>(action: () => Promise<T>): Promise<IpcResult<T>> => {
    try { return { ok: true, data: await action() } }
    catch (error) { return { ok: false, error: { code: 'MCP_UNAVAILABLE', message: error instanceof Error ? error.message : '无法连接本地 MCP 服务，请确认 WebTools 托盘仍在运行。' } } }
  }
  const status = async (): Promise<McpGatewayStatus> => {
    const result = parseMcpGatewayStatus(await deps.request('mcp-admin-get-v1', {}))
    if (!result) throw Error('无法读取本地 MCP 服务状态。')
    return result
  }
  manager.handle(IPC_CHANNELS.pluginMcpGet, () => protect(status))
  manager.handle(IPC_CHANNELS.pluginMcpTools, () => protect(async () => deps.listTools()))
  manager.handle(IPC_CHANNELS.pluginMcpPreferences, () => protect(async () => {
    const result = await deps.request('mcp-admin-preferences-v1', {})
    if (!result || typeof result !== 'object' || !('disabledPlugins' in result) || !Array.isArray(result.disabledPlugins) || !result.disabledPlugins.every(id => typeof id === 'string')) throw Error('无法读取 MCP 插件开关。')
    return { disabledPlugins: result.disabledPlugins as string[] }
  }))
  manager.handle(IPC_CHANNELS.pluginMcpSetEnabled, (_event, enabled) => protect(async () => {
    if (typeof enabled !== 'boolean') throw Error('无效开关。')
    const result = parseMcpGatewayStatus(await deps.request('mcp-admin-set-enabled-v1', { enabled }))
    if (!result) throw Error('无法读取本地 MCP 服务状态。')
    return result
  }))
  manager.handle(IPC_CHANNELS.pluginMcpPluginEnabled, (_event, id, enabled) => protect(async () => {
    if (typeof id !== 'string' || typeof enabled !== 'boolean' || !deps.listTools().some(tool => tool.pluginId === id)) throw Error('未知的插件工具。')
    await deps.request('mcp-admin-set-plugin-enabled-v1', { pluginId: id, enabled }); return null
  }))
  manager.handle(IPC_CHANNELS.pluginMcpAgents, () => protect(async () => {
    const value = await status()
    if (!value.running) throw Error('请先开启本地 MCP 服务。')
    return deps.agents.list(value.url, value.token)
  }))
  manager.handle(IPC_CHANNELS.pluginMcpConfigureAgent, (_event, id) => protect(async () => {
    const value = await status()
    if (!value.running || typeof id !== 'string') throw Error('请先开启本地 MCP 服务。')
    try { await deps.agents.install(id, value.url, value.token) }
    catch { throw Error('无法写入 Agent 配置，请检查文件权限或格式。原配置未被替换。') }
    return null
  }))
}
