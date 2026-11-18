<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { Clock, RefreshCw, Trash2 } from '@lucide/vue'
import type { ManagedScheduledTask } from '../../shared/ipc.ts'
import DeleteConfirmationDialog from '../favorites/DeleteConfirmationDialog.vue'
import { scheduledTaskIsActive, scheduledTaskLogs, scheduledTaskRule, scheduledTaskState, scheduledTaskTime } from './scheduled-task-view-model.ts'

const tasks = ref<ManagedScheduledTask[]>([])
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const message = ref('')
const selected = ref<ManagedScheduledTask | null>(null)
const deleting = ref(false)
defineExpose({ busy: deleting })
const deleteError = ref('')
const now = ref(Date.now())
const active = computed(() => tasks.value.filter(task => scheduledTaskIsActive(task, now.value)))
const history = computed(() => tasks.value.filter(task => !scheduledTaskIsActive(task, now.value)))
const groups = computed(() => [
  { title: '待执行与执行中', tasks: active.value },
  { title: '已结束与暂停记录', tasks: history.value },
].filter(group => group.tasks.length))
let mounted = false
let timer: number | undefined

async function refresh(): Promise<void> {
  // A slow list request must not overwrite a newer deletion result.
  if (!mounted || loading.value || deleting.value) return
  loading.value = true
  try {
    const result = await window.desktop.pluginTasks.list()
    if (!mounted) return
    if (result.ok) { tasks.value = result.data; loaded.value = true; error.value = ''; now.value = Date.now() }
    else error.value = result.error.message
  } catch { if (mounted) error.value = '无法读取定时任务，请确认 WebTools 宿主正在运行后重试。' }
  finally { if (mounted) loading.value = false }
}

function visibilityChanged(): void {
  if (document.visibilityState === 'visible') void refresh()
}
onMounted(() => {
  mounted = true
  void refresh()
  timer = window.setInterval(() => {
    if (document.visibilityState === 'visible') void refresh()
  }, 5000)
  document.addEventListener('visibilitychange', visibilityChanged)
})
onBeforeUnmount(() => {
  mounted = false
  if (timer !== undefined) window.clearInterval(timer)
  document.removeEventListener('visibilitychange', visibilityChanged)
})

function requestDelete(task: ManagedScheduledTask): void {
  if (loading.value || deleting.value) return
  selected.value = task
  deleteError.value = ''
}
async function remove(): Promise<void> {
  const task = selected.value
  if (!task || deleting.value) return
  deleting.value = true
  deleteError.value = ''
  let removed = false
  try {
    const result = await window.desktop.pluginTasks.remove(task.pluginId, task.code)
    if (!mounted) return
    if (!result.ok) { deleteError.value = result.error.message; return }
    tasks.value = tasks.value.filter(row => row.pluginId !== task.pluginId || row.code !== task.code)
    selected.value = null
    message.value = result.data ? '任务已删除。' : '任务已不存在，列表已更新。'
    removed = true
  } catch { if (mounted) deleteError.value = '删除失败，请重试。' }
  finally { if (mounted) deleting.value = false }
  if (removed) await refresh()
}
</script>

<template>
  <section class="scheduled-tasks" aria-label="插件定时任务">
    <header class="tasks-heading">
      <div><h2><Clock :size="19" aria-hidden="true" />定时任务</h2><p>统一查看插件登记的任务和执行结果。WebTools 在后台按计划运行，退出 WebTools 后停止调度。</p></div>
      <button type="button" class="secondary-button" :disabled="loading || deleting" @click="refresh"><RefreshCw :size="14" aria-hidden="true" />{{ loading ? '读取中…' : '刷新' }}</button>
    </header>
    <p class="tasks-summary">{{ active.length }} 个待执行或执行中任务 · {{ history.length }} 条已结束或暂停记录</p>
    <p v-if="error" class="inline-error" role="alert">{{ error }}</p>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="!loaded && loading" role="status">正在读取任务…</p>
    <div v-if="loaded && !tasks.length" class="tasks-empty"><Clock :size="30" aria-hidden="true" /><h3>暂无定时任务</h3><p>在支持定时任务的插件中创建任务，并在 WebTools 确认后，任务会显示在这里。</p></div>
    <section v-for="group in groups" :key="group.title" class="task-group" :aria-label="group.title">
      <h3>{{ group.title }}</h3>
      <article v-for="task in group.tasks" :key="JSON.stringify([task.pluginId, task.code])" class="task-card">
        <div class="task-heading"><div><h4>{{ task.description }}</h4><p class="plugin-name">{{ task.pluginName }} <span>· {{ task.code }}</span></p></div><span class="task-state" :class="{ running: task.running }">{{ scheduledTaskState(task, now) }}</span></div>
        <p class="task-rule">{{ scheduledTaskRule(task) }}</p>
        <dl class="task-facts"><div><dt>下次执行</dt><dd>{{ task.enabled ? scheduledTaskTime(task.nextRunMs) : '—' }}</dd></div><div><dt>最近执行</dt><dd>{{ scheduledTaskTime(task.lastRunMs) }}</dd></div><div><dt>执行结果</dt><dd>成功 {{ task.successes }} · 失败 {{ task.failures }}</dd></div></dl>
        <p v-if="task.lastError" class="task-error">最近错误：{{ task.lastError }}</p>
        <details class="task-logs"><summary>执行日志（最近 {{ task.logs?.length ?? 0 }} 条）</summary>
          <p v-if="!task.logs?.length">暂无执行日志。此前版本的统计记录会继续保留；新执行的任务会记录时间、结果和耗时。</p>
          <ol v-else class="execution-list" aria-label="最近执行日志">
            <li v-for="(log, index) in scheduledTaskLogs(task)" :key="`${log.executeAtMs}-${index}`">
              <div class="execution-heading"><time :datetime="new Date(log.executeAtMs).toISOString()">{{ scheduledTaskTime(log.executeAtMs) }}</time><strong :class="{ 'task-error': !log.success }">{{ log.success ? '成功' : '失败' }}</strong><span>耗时 {{ log.durationMs }} 毫秒</span></div>
              <p v-if="log.error" class="task-error">{{ log.error }}</p>
            </li>
          </ol>
        </details>
        <details class="task-identity"><summary>任务详情</summary><p>插件 ID：{{ task.pluginId }}<br>功能 ID：{{ task.featureId }}<br>创建于：{{ scheduledTaskTime(task.createdAtMs) }}</p></details>
        <footer><button type="button" class="secondary-button" :disabled="loading || deleting" :aria-label="`删除 ${task.pluginName} 的 ${task.description}`" @click="requestDelete(task)"><Trash2 :size="14" aria-hidden="true" />删除任务</button></footer>
      </article>
    </section>
    <p v-if="history.length" class="tasks-note">一次性任务结束后保留统计记录；暂停的任务需要在原插件中删除并重新确认创建。删除记录后无法在此查看其统计。</p>
    <Teleport to="body"><DeleteConfirmationDialog v-if="selected" title="删除定时任务？" :message="`将删除「${selected.pluginName}」的「${selected.description}」及其统计记录，并取消后续执行。已经开始执行的操作不会撤销。`" :saving="deleting" :error="deleteError" @confirm="remove" @cancel="selected = null" /></Teleport>
  </section>
</template>

<style scoped>
.scheduled-tasks { display: grid; gap: var(--manager-unit); }
.tasks-heading, .task-heading { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--manager-unit); }
.tasks-heading > div, .task-heading > div { min-width: 0; }
h2 { display: flex; align-items: center; gap: 9px; margin: 0; font-size: var(--manager-font-heading, 18px); }
h3 { margin: 0; font-size: var(--manager-font-body); } h4 { margin: 0; font-size: var(--manager-font-body); overflow-wrap: anywhere; }
p { margin: 8px 0 0; line-height: 1.6; color: var(--muted); }
.tasks-summary { margin: 0; }.tasks-heading button { flex-shrink: 0; }
button { display: inline-flex; align-items: center; justify-content: center; gap: 7px; }
button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
.task-group { display: grid; gap: 12px; }
.task-card { min-width: 0; padding: calc(var(--manager-unit) * 1.25); border: 1px solid var(--line); border-radius: 12px; background: var(--surface-raised); }
.plugin-name { color: var(--text); }.plugin-name span, .task-identity { color: var(--muted); }
.task-state { flex-shrink: 0; padding: 4px 9px; border-radius: 6px; color: var(--muted); background: var(--hover); font-size: var(--manager-font-body); }.task-state.running { color: var(--accent); background: var(--accent-soft); }
.task-rule, .task-error, .task-identity p { overflow-wrap: anywhere; }
.task-rule { font-family: inherit; }
.task-facts { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; margin: 16px 0; }.task-facts dt { color: var(--muted); margin-bottom: 5px; }.task-facts dd { margin: 0; overflow-wrap: anywhere; }
.task-error { color: var(--danger, #e68d8d); }.task-identity summary, .task-logs summary { width: fit-content; cursor: pointer; }
.task-logs { margin-block: 12px; color: var(--muted); }
.execution-list { list-style: none; margin: 12px 0 0; padding: 0; }
.execution-list li { border-top: 1px solid var(--line); padding-block: 10px; }
.execution-heading { display: flex; flex-wrap: wrap; gap: 8px 16px; }.execution-heading strong { color: var(--text); }.execution-heading strong.task-error { color: var(--danger, #e68d8d); }
.execution-list p { overflow-wrap: anywhere; }
footer { display: flex; justify-content: flex-end; margin-top: 12px; }
.tasks-empty { display: grid; justify-items: center; text-align: center; padding: 48px 24px; border: 1px solid var(--line); border-radius: 12px; color: var(--muted); }.tasks-empty h3 { margin-top: 16px; color: var(--text); }.tasks-note { margin: 0; }
@media (max-width: 640px) { .task-facts { grid-template-columns: 1fr; }.tasks-heading { flex-wrap: wrap; } }
</style>
