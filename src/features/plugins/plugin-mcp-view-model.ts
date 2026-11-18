import type { McpGatewayStatus } from '../../shared/plugin-mcp-contracts.ts'
export interface McpPanelState { enabled: boolean; label: string; canConnect: boolean; canRetry: boolean; catalogLabel: string }
export function toMcpPanelState(status: McpGatewayStatus | null, loadError = ''): McpPanelState {
  const label = loadError || (!status ? '正在读取本地 MCP 服务状态…' : status.error?.message
    || (status.running ? '服务运行中。关闭管理窗口后仍可调用；退出 WebTools 后服务停止。'
      : status.phase === 'starting' ? '正在启动本地 MCP 服务…' : '开启后可连接本机 AI Agent，服务使用固定地址和访问令牌。'))
  return { enabled: status?.enabled ?? false, label, canConnect: !!status?.running && !loadError,
    canRetry: status?.phase === 'error' && !loadError,
    catalogLabel: status && !status.catalogReady ? '正在同步插件工具…' : '' }
}
