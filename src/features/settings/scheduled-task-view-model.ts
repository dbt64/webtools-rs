import type { ManagedScheduledTask } from '../../shared/ipc.ts'

export function scheduledTaskState(task: ManagedScheduledTask, now = Date.now()): string {
  if (task.running) return '执行中'
  if (task.schedule.kind === 'once' && !task.enabled) {
    return task.lastRunMs == null && task.lastError ? '已暂停' : '已结束'
  }
  if (task.schedule.kind === 'cron' && task.schedule.endMs != null && now > task.schedule.endMs) return '已到期'
  if (!task.enabled) return '已暂停'
  return task.nextRunMs == null ? '已结束' : '等待执行'
}

export function scheduledTaskTime(value: number | null | undefined): string {
  if (value == null) return '—'
  const date = new Date(value)
  return Number.isFinite(date.getTime()) ? date.toLocaleString() : '时间无效'
}

export function scheduledTaskRule(task: ManagedScheduledTask): string {
  if (task.schedule.kind === 'once') return `一次性 · ${scheduledTaskTime(task.schedule.atMs)}`
  const { expression, startMs, endMs } = task.schedule
  return [`Cron · ${expression}`, startMs == null ? '' : `开始 ${scheduledTaskTime(startMs)}`,
    endMs == null ? '' : `截止 ${scheduledTaskTime(endMs)}`].filter(Boolean).join(' · ')
}

export function scheduledTaskIsActive(task: ManagedScheduledTask, now = Date.now()): boolean {
  return task.running || (task.enabled && task.nextRunMs != null
    && !(task.schedule.kind === 'cron' && task.schedule.endMs != null && now > task.schedule.endMs))
}

export function scheduledTaskLogs(task: ManagedScheduledTask) {
  return [...(task.logs ?? [])].sort((a, b) => b.executeAtMs - a.executeAtMs)
}
