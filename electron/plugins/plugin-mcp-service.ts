import { randomUUID, timingSafeEqual } from 'node:crypto'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js'
import { CallToolRequestSchema, ListToolsRequestSchema, type CallToolResult } from '@modelcontextprotocol/sdk/types.js'
type PluginMcpStatus = { running: false } | { running: true; url: string; token: string }
import type { NodeCommandExecutionAdapter } from './node-command-execution.ts'
import type { PluginManager } from './plugin-manager.ts'
import { McpToolExecution } from './mcp-tool-execution.ts'
import type { McpSettingsStore } from './mcp-settings.ts'

const loopback = '127.0.0.1'
const maxRequestBytes = 64 * 1024
const toolError = (text: string): CallToolResult => ({ isError: true, content: [{ type: 'text', text }] })
const toolName = (tool: { pluginId: string; featureId: string }) => `${tool.pluginId}.${tool.featureId}`

export class PluginMcpService {
  private session: { stop(): void; ready: Promise<PluginMcpStatus> } | undefined
  private status: PluginMcpStatus = { running: false }
  private readonly execution: McpToolExecution

  constructor(private readonly manager: PluginManager, private readonly commands: NodeCommandExecutionAdapter,
    private readonly hostVersion: string, private readonly settings: McpSettingsStore) {
    this.execution = new McpToolExecution(manager, commands, id => !settings.value.disabledPlugins.includes(id))
  }

  getStatus(): PluginMcpStatus { return this.status }

  setEnabled(enabled: boolean): Promise<PluginMcpStatus> {
    if (!enabled) {
      this.close()
      return Promise.resolve(this.status)
    }
    if (this.session) return this.session.ready

    const token = this.settings.value.token
    const authorization = Buffer.from(`Bearer ${token}`)
    const controller = new AbortController()
    const http = createServer({ requestTimeout: 15_000 }, (request, response) => {
      void this.handle(request, response, authorization, controller.signal).catch(() => {
        if (!response.headersSent && !response.destroyed) response.writeHead(500).end()
        else if (!response.writableEnded) response.destroy()
      })
    })
    const stop = () => {
      controller.abort()
      http.close()
      http.closeAllConnections()
    }
    const ready = new Promise<PluginMcpStatus>((resolve, reject) => {
      http.once('error', reject)
      http.listen(this.settings.value.port, loopback, () => {
        const address = http.address()
        if (controller.signal.aborted || !address || typeof address === 'string') {
          reject(new Error('MCP service stopped.'))
          return
        }
        this.status = { running: true, url: `http://${loopback}:${address.port}/mcp`, token }
        resolve(this.status)
      })
    }).catch(error => {
      if (this.session?.ready === ready) this.close()
      throw error
    })
    this.session = { stop, ready }
    return ready
  }

  listTools() { return this.manager.listMcpTools().filter(tool => !this.settings.value.disabledPlugins.includes(tool.pluginId)) }

  close(): void {
    const session = this.session
    this.session = undefined
    this.status = { running: false }
    session?.stop()
  }

  private async handle(request: IncomingMessage, response: ServerResponse, authorization: Buffer, signal: AbortSignal) {
    if (!this.status.running || signal.aborted) { response.writeHead(503).end(); return }
    const url = new URL(this.status.url)
    if (request.headers.origin !== undefined || request.headers.host !== url.host) { response.writeHead(403).end(); return }
    if (request.url !== url.pathname) { response.writeHead(404).end(); return }
    const supplied = Buffer.from(request.headers.authorization ?? '')
    if (supplied.length !== authorization.length || !timingSafeEqual(supplied, authorization)) {
      response.writeHead(401, { 'WWW-Authenticate': 'Bearer' }).end()
      return
    }
    if (request.method !== 'POST') { response.writeHead(405, { Allow: 'POST' }).end(); return }

    const chunks: Buffer[] = []
    let length = 0
    for await (const chunk of request) {
      length += chunk.length
      if (length > maxRequestBytes) { response.writeHead(413).end(); return }
      chunks.push(chunk)
    }
    let body: unknown
    try { body = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks))) }
    catch {
      response.writeHead(400, { 'Content-Type': 'application/json' }).end(JSON.stringify({
        jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' },
      }))
      return
    }

    const server = new Server({ name: 'WebTools Plugin Tools', version: this.hostVersion }, { capabilities: { tools: {} } })
    server.setRequestHandler(ListToolsRequestSchema, async () => ({
      tools: this.listTools().map(tool => ({
        name: toolName(tool), title: tool.title, description: tool.description, inputSchema: { ...tool.inputSchema, type: 'object' as const },
        ...(tool.outputSchema ? { outputSchema: { ...tool.outputSchema, type: 'object' as const } } : {}),
      })),
    }))
    server.setRequestHandler(CallToolRequestSchema, (request, extra) =>
      this.callTool(request.params.name, request.params.arguments ?? {}, AbortSignal.any([signal, extra.signal])))
    const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true })
    response.once('close', () => { void server.close() })
    await server.connect(transport)
    await transport.handleRequest(request, response, body)
  }

  private async callTool(name: string, input: Record<string, unknown>, signal: AbortSignal): Promise<CallToolResult> {
    if (signal.aborted) return toolError('工具调用已取消。')
    const callId = randomUUID()
    const cancel = () => this.execution.cancel(callId)
    signal.addEventListener('abort', cancel, { once: true })
    try {
      const result = await this.execution.execute({ callId, toolName: name, arguments: input, deadlineUnixMs: Date.now() + 60_000 })
      return result.result ? { ...result.result } : toolError('工具调用已取消。')
    } finally { signal.removeEventListener('abort', cancel) }
  }
}
