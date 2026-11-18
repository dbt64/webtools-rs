<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue'
import type { PluginMcpStatus } from '../../shared/ipc.ts'
import { toMcpPanelState } from './plugin-mcp-view-model.ts'
const status = ref<PluginMcpStatus | null>(null)
const busy = ref(false)
const message = ref('')
const loadError = ref('')
const panel = computed(() => toMcpPanelState(status.value, loadError.value))
const connectButton = ref<HTMLButtonElement | null>(null)
const connectDialog = ref<HTMLElement | null>(null)
let generation = 0
let alive = true
const connecting = ref(false)
const disabled = ref<string[]>([])
const tools = ref<Array<{ pluginId: string; featureId: string; title: string; description: string }>>([])
const plugins = ref<Array<{ id: string; name: string }>>([])
const agents = ref<Array<{ id: string; name: string; path: string; configured: boolean }>>([])
const groups = computed(() => [...new Set(tools.value.map(t => t.pluginId))].map(id => ({
  id, name: plugins.value.find(p => p.id === id)?.name ?? id, tools: tools.value.filter(t => t.pluginId === id),
})))
const enabledGroups = computed(() => groups.value.filter(p => !disabled.value.includes(p.id)))
const config = computed(() => status.value?.running ? JSON.stringify({ mcpServers: { webtools: {
  type: 'http', url: status.value.url, headers: { Authorization: `Bearer ${status.value.token}` },
} } }, null, 2) : '')
async function load() {
  const current = ++generation
  busy.value = true
  try {
    const [available, state, preferences, installed] = await Promise.all([
      window.desktop.pluginMcp.listTools(), window.desktop.pluginMcp.get(), window.desktop.pluginMcp.preferences(), window.desktop.plugins.list(),
    ])
    if (!alive || current !== generation) return
    loadError.value = ''
    for (const result of [available, state, preferences, installed]) if (!result.ok) loadError.value ||= result.error.message
    if (available.ok) tools.value = available.data
    if (state.ok) status.value = state.data
    if (preferences.ok) disabled.value = preferences.data.disabledPlugins
    if (installed.ok) plugins.value = installed.data
  } catch { if (alive && current === generation) loadError.value = '无法读取本地 MCP 服务状态。' }
  finally { if (alive && current === generation) busy.value = false }
}
const refreshOnFocus = () => { if (!busy.value && !connecting.value) void load() }
onMounted(() => { void load(); window.addEventListener('focus', refreshOnFocus) })
onUnmounted(() => { alive = false; generation++; window.removeEventListener('focus', refreshOnFocus) })
async function toggle(enabled = !status.value?.enabled) {
  const current = ++generation
  busy.value = true
  try {
    const result = await window.desktop.pluginMcp.setEnabled(enabled)
    if (!alive || current !== generation) return
    if (result.ok) { status.value = result.data; message.value = ''; loadError.value = '' }
    else { message.value = result.error.message; await load() }
  } catch { if (alive && current === generation) message.value = '无法连接本地 MCP 服务，请确认 WebTools 托盘仍在运行。' }
  finally { if (alive && current === generation) busy.value = false }
}
async function togglePlugin(id: string) {
  const current = ++generation
  busy.value = true
  try {
    const result = await window.desktop.pluginMcp.setPluginEnabled(id, disabled.value.includes(id))
    if (!alive || current !== generation) return
    if (result.ok) await load()
    else message.value = result.error.message
  } catch { if (alive && current === generation) message.value = '无法保存 MCP 插件开关。' }
  finally { if (alive && current === generation) busy.value = false }
}
async function connect() {
  connecting.value = true
  await nextTick(); connectDialog.value?.querySelector<HTMLButtonElement>('button')?.focus()
  try {
    const result = await window.desktop.pluginMcp.agents()
    if (!alive || !connecting.value) return
    if (result.ok) agents.value = result.data
    else message.value = result.error.message
  } catch { if (alive && connecting.value) message.value = '无法读取 Agent 配置。' }
}
function closeConnect() { connecting.value = false; void nextTick(() => connectButton.value?.focus()) }
function dialogKey(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeConnect(); return }
  if (event.key !== 'Tab') return
  const items = [...(connectDialog.value?.querySelectorAll<HTMLElement>('button:not(:disabled), textarea, summary, a[href]') ?? [])].filter(item => item.getClientRects().length)
  const first = items[0], last = items.at(-1)
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus() }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
}
async function configure(id: string) {
  busy.value = true
  try {
    const result = await window.desktop.pluginMcp.configureAgent(id)
    if (!alive) return
    if (result.ok) { message.value = '配置已写入，原文件已备份。请在 Agent 中重连 MCP，必要时重启 Agent。'; await connect() }
    else message.value = result.error.message
  } catch { if (alive) message.value = '无法写入 Agent 配置，原配置未被替换。' }
  finally { if (alive) busy.value = false }
}
async function copyConfig() {
  try { await navigator.clipboard.writeText(config.value); message.value = '连接配置已复制。' }
  catch { message.value = '请从下方手动复制连接配置。' }
}
</script>
<template>
  <section class="mcp-settings">
    <div class="service-card">
      <div><h2>本地 MCP 服务</h2><p>让 AI Agent 调用已安装插件提供的工具。</p></div>
      <button ref="connectButton" class="secondary-button" :disabled="!panel.canConnect || busy" @click="connect">连接 AI Agent</button>
      <button class="switch" role="switch" :aria-checked="panel.enabled" aria-label="本地 MCP 服务" :disabled="busy || !status" @click="toggle()"><span /></button>
    </div>
    <p class="muted" role="status">{{ panel.label }} <button v-if="panel.canRetry" class="secondary-button" :disabled="busy" @click="toggle(true)">重试</button></p>
    <div class="tools-card">
      <div class="tools-heading"><p>已启用 {{ enabledGroups.length }} 个插件，共 {{ enabledGroups.reduce((n, p) => n + p.tools.length, 0) }} 个 AI Agent 可用工具</p><button class="secondary-button" :disabled="busy" @click="load">刷新</button></div>
      <div v-for="plugin in groups" :key="plugin.id" class="plugin-row">
        <div class="plugin-icon">{{ plugin.name.slice(0, 1) }}</div>
        <div class="plugin-info"><strong>{{ plugin.name }}</strong><details><summary>{{ plugin.tools.length }} 个工具</summary><p v-for="tool in plugin.tools" :key="tool.featureId">{{ tool.title }} · {{ tool.description }}</p></details></div>
        <button class="switch" role="switch" :aria-checked="!disabled.includes(plugin.id)" :aria-label="plugin.name + ' 工具'" :disabled="busy" @click="togglePlugin(plugin.id)"><span /></button>
      </div>
      <p v-if="panel.catalogLabel">{{ panel.catalogLabel }}</p>
      <p v-else-if="!groups.length && !busy && !loadError">暂无插件提供工具。服务是 WebTools 内置能力；安装声明了 MCP 工具的业务插件后会显示在这里。</p>
    </div>
    <div v-if="connecting" class="connect-overlay" @click.self="closeConnect">
      <section ref="connectDialog" class="connect-dialog" role="dialog" aria-modal="true" aria-label="连接 AI Agent" @keydown="dialogKey">
        <div class="tools-heading"><h2>连接 AI Agent</h2><button class="secondary-button" @click="closeConnect">关闭</button></div>
        <details :open="!agents.length"><summary>MCP 配置 · 手动导入</summary>
          <p>复制完整 JSON，添加到支持 Streamable HTTP 的 Agent。也可单独填写 URL 和 Authorization 请求头。</p>
          <textarea readonly rows="11" :value="config" aria-label="MCP 连接配置" spellcheck="false" />
          <button class="secondary-button" @click="copyConfig">复制连接配置</button>
        </details>
        <p>检测到以下 AI Agent，可一键写入 MCP 配置。写入前自动备份，保留其他配置值。</p>
        <div v-for="agent in agents" :key="agent.id" class="agent-row"><div><strong>{{ agent.name }}</strong><p>{{ agent.path }}</p></div><button class="primary-button" :disabled="busy || agent.configured" @click="configure(agent.id)">{{ agent.configured ? '已配置' : '一键配置' }}</button></div>
        <p v-if="!agents.length">未检测到支持的本机 Agent 配置，请使用上方手动配置。</p>
        <p v-if="message" role="status">{{ message }}</p>
      </section>
    </div>
    <p v-if="message && !connecting" role="status">{{ message }}</p>
  </section>
</template>
<style scoped>
.mcp-settings { margin-block: 16px 28px; }
h2 { font-size: 18px; margin: 0 0 8px; } p { color: var(--muted); line-height: 1.6; }
.service-card,.tools-card { padding: 24px; background: var(--surface-raised, #202124); border: 1px solid var(--line); border-radius: 16px; }
.service-card,.tools-heading,.plugin-row,.agent-row { display: flex; gap: 20px; align-items: center; }
.service-card > div:first-child,.plugin-info,.agent-row > div { flex: 1; min-width: 0; }
.service-card p { margin-bottom: 0; }.tools-heading { justify-content: space-between; }
.plugin-row { padding: 18px 0; border-top: 1px solid var(--line); }
.plugin-icon { width: 44px; height: 44px; display: grid; place-items: center; border-radius: 10px; background: var(--selected, #2B2C30); font-size: 22px; }
summary { cursor: pointer; color: var(--muted); margin-block: 6px; }
.switch { width: 42px; height: 24px; flex-shrink: 0; border: 0; border-radius: 20px; background: #64646b; padding: 3px; cursor: pointer; }
.switch span { display: block; width: 18px; height: 18px; border-radius: 50%; background: #f4f4f5; }
.switch[aria-checked="true"] { background: #90b8ff; }.switch[aria-checked="true"] span { margin-left: 18px; background: #17243b; }
.connect-overlay { position: fixed; inset: 0; z-index: 100; display: grid; place-items: center; background: #0008; padding: 24px; }
.connect-dialog { background: var(--surface, #18191B); color: var(--text); border: 1px solid var(--line); border-radius: 18px; padding: 28px; width: min(760px, 90vw); max-height: 85vh; overflow: auto; }
textarea { box-sizing: border-box; width: 100%; padding: 16px; background: var(--surface); color: inherit; border: 1px solid var(--line); border-radius: 8px; font-family: monospace; }
.agent-row { padding: 16px 0; border-top: 1px solid var(--line); }.agent-row p { overflow-wrap: anywhere; margin: 4px 0; }
</style>
