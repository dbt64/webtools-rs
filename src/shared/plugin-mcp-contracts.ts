export interface McpCallRequest { callId: string; toolName: string; arguments: Record<string, unknown>; deadlineUnixMs: number }
export interface McpToolProjection { pluginId: string; featureId: string; title: string; description: string; inputSchema: Record<string, unknown>; outputSchema?: Record<string, unknown> }
export interface McpToolResult { content: Array<{ type: 'text'; text: string }>; isError?: boolean; structuredContent?: Record<string, unknown> }
export interface McpCallReply { callId: string; status: 'completed' | 'cancelled' | 'failed' | 'outcome-unknown'; result?: McpToolResult; error?: string }
interface GatewayState { enabled: boolean; phase: 'disabled' | 'starting' | 'running' | 'error'; catalogReady: boolean; activeCalls: number; error?: { code: string; message: string } }
export type McpGatewayStatus = GatewayState & ({ running: false } | { running: true; url: string; token: string })
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v)
export function parseMcpGatewayStatus(value: unknown): McpGatewayStatus | null {
  if (!record(value) || typeof value.enabled !== 'boolean' || typeof value.running !== 'boolean' || typeof value.catalogReady !== 'boolean'
    || !Number.isInteger(value.activeCalls) || Number(value.activeCalls) < 0 || Number(value.activeCalls) > 8
    || !['disabled', 'starting', 'running', 'error'].includes(String(value.phase))) return null
  if (value.running !== (value.phase === 'running') || value.running && !value.enabled) return null
  if (value.error !== undefined && (!record(value.error) || typeof value.error.code !== 'string' || typeof value.error.message !== 'string')) return null
  if (value.running) {
    if (typeof value.url !== 'string' || typeof value.token !== 'string' || !/^[A-Za-z0-9_-]{32,128}$/.test(value.token)) return null
    try { const url = new URL(value.url); if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || url.pathname !== '/mcp' || url.username || url.password || url.search || url.hash) return null } catch { return null }
  }
  return value as unknown as McpGatewayStatus
}
