import type { McpCallRequest, McpToolProjection } from '../../src/shared/plugin-mcp-contracts.ts'
import type { McpToolExecution } from '../plugins/mcp-tool-execution.ts'
import type { NativeManagerClient } from './native-manager-client.ts'
import { nativeManagerReply } from './native-manager-client.ts'
import { MAX_PROTOCOL_MESSAGE_BYTES, NativeManagerRequestError, type NativeManagerEnvelope } from './native-manager-protocol.ts'

export class NativeMcpGateway {
  private revision = 0
  private closed = false
  private publishQueue: Promise<void> = Promise.resolve()
  constructor(private readonly client: Pick<NativeManagerClient, 'supportsMcpGateway' | 'request'>,
    private readonly execution: Pick<McpToolExecution, 'execute' | 'cancel' | 'close'>) {}
  publishTools(tools: McpToolProjection[]): Promise<void> {
    if (this.closed || !this.client.supportsMcpGateway) return Promise.resolve()
    const payload = { schemaVersion: 1, revision: ++this.revision, tools: structuredClone(tools) }
    if (Buffer.byteLength(JSON.stringify(payload)) + 1024 > MAX_PROTOCOL_MESSAGE_BYTES) return Promise.reject(Error('MCP 工具列表过大，无法同步。'))
    const current = this.publishQueue.then(async () => {
      if (this.closed) return
      await this.client.request('mcp-tools-update-v1', payload)
    })
    this.publishQueue = current.catch(() => {})
    return current
  }
  async handleCommand(message: NativeManagerEnvelope) {
    if (!['mcp-call-v1', 'mcp-call-cancel-v1', 'mcp-sync-v1'].includes(message.type)) return
    if (this.closed || !this.client.supportsMcpGateway) throw new NativeManagerRequestError('MCP_UNAVAILABLE', 'MCP 调用连接已失效。')
    const payload = message.payload
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new NativeManagerRequestError('INVALID_REQUEST', '无效 MCP 请求。')
    const data = payload as Record<string, unknown>
    if (message.type === 'mcp-call-cancel-v1') {
      if (typeof data.callId !== 'string') throw new NativeManagerRequestError('INVALID_REQUEST', '无效取消请求。')
      this.execution.cancel(data.callId)
      return nativeManagerReply({ cancelled: true })
    }
    if (message.type === 'mcp-sync-v1') return
    if (typeof data.callId !== 'string' || !data.callId || typeof data.toolName !== 'string'
      || !data.arguments || typeof data.arguments !== 'object' || Array.isArray(data.arguments)
      || typeof data.deadlineUnixMs !== 'number' || !Number.isSafeInteger(data.deadlineUnixMs)) throw new NativeManagerRequestError('INVALID_REQUEST', '无效 MCP 调用。')
    const result = await this.execution.execute(payload as unknown as McpCallRequest)
    if (Buffer.byteLength(JSON.stringify(result)) + 1024 > MAX_PROTOCOL_MESSAGE_BYTES) return nativeManagerReply({ callId: data.callId, status: 'failed', error: '插件返回值超过通信大小限制。' })
    return nativeManagerReply(result)
  }
  close(): void { this.closed = true; this.execution.close() }
}
