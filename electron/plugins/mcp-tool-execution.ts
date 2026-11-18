import type { McpCallRequest, McpCallReply, McpToolResult } from '../../src/shared/plugin-mcp-contracts.ts'
import type { PluginManager } from './plugin-manager.ts'
import type { NodeCommandExecutionAdapter } from './node-command-execution.ts'
import { compileToolSchema } from './mcp-tool-schema.ts'
import { safePluginError } from './errors.ts'
const toolError = (text: string): McpToolResult => ({ isError: true, content: [{ type: 'text', text }] })

/** Runs authorized tools; an aborted waiter does not release the underlying execution. */
export class McpToolExecution {
  private readonly active = new Map<string, { controller: AbortController; completion: Promise<McpCallReply> }>()
  private closed = false
  constructor(private readonly manager: PluginManager, private readonly commands: Pick<NodeCommandExecutionAdapter, 'execute'>,
    private readonly isMcpPluginEnabled: (pluginId: string) => boolean) {}
  get activeCalls(): number { return this.active.size }
  execute(request: McpCallRequest): Promise<McpCallReply> {
    const pending = this.active.get(request.callId)
    if (pending) return pending.completion
    if (this.closed || request.deadlineUnixMs <= Date.now()) return Promise.resolve({ callId: request.callId, status: 'cancelled' })
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), Math.min(60_000, request.deadlineUnixMs - Date.now()))
    const completion = this.run(request, controller.signal).finally(() => { clearTimeout(timer); this.active.delete(request.callId) })
    this.active.set(request.callId, { controller, completion })
    return completion
  }
  cancel(callId: string): void { this.active.get(callId)?.controller.abort() }
  close(): void { this.closed = true; for (const work of this.active.values()) work.controller.abort() }
  private async run(request: McpCallRequest, signal: AbortSignal): Promise<McpCallReply> {
    const reply = (result: McpToolResult): McpCallReply => ({ callId: request.callId, status: 'completed', result })
    const tool = this.manager.listMcpTools().find(tool => `${tool.pluginId}.${tool.featureId}` === request.toolName && this.isMcpPluginEnabled(tool.pluginId))
    if (!tool) return reply(toolError('该工具不存在，或所属插件当前不可用。'))
    try {
      let result = toolError('工具调用已取消。')
      const { completion } = await this.manager.authorizeWebFeature({ pluginId: tool.pluginId, featureId: tool.featureId }, null, async authority => {
        try {
          const manifest = authority.snapshot.manifest
          if (manifest.manifestVersion !== 3 || signal.aborted || !this.isMcpPluginEnabled(tool.pluginId)) return
          const schema = manifest.features.find(feature => feature.id === tool.featureId)?.mcpTool
          if (!schema) return
          if (!compileToolSchema(schema.inputSchema)(request.arguments)) { result = toolError('工具参数与 inputSchema 不匹配。'); return }
          const outcome = await this.commands.execute(authority, undefined, { mcpInput: request.arguments, signal })
          if (outcome.status !== 'completed' || !outcome.mcpResult || signal.aborted) return
          if (schema.outputSchema && !compileToolSchema(schema.outputSchema)(outcome.mcpResult)) { result = toolError('插件返回值与 outputSchema 不匹配。'); return }
          result = { structuredContent: outcome.mcpResult, content: [{ type: 'text', text: JSON.stringify(outcome.mcpResult) }] }
        } finally { authority.revoke() }
      }, 'mcp')
      await completion
      return signal.aborted ? { callId: request.callId, status: 'cancelled' } : reply(result)
    } catch (error) {
      return signal.aborted ? { callId: request.callId, status: 'cancelled' } : reply(toolError(safePluginError(error).message))
    }
  }
}
