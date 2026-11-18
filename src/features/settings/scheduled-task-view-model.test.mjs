import assert from 'node:assert/strict'
import { test } from 'node:test'
import { scheduledTaskIsActive, scheduledTaskLogs, scheduledTaskRule, scheduledTaskState, scheduledTaskTime } from './scheduled-task-view-model.ts'

const task = (overrides = {}) => ({
  pluginId: 'org.example.reminder', pluginName: '提醒', featureId: 'reminder', code: 'daily', description: '喝水提醒',
  schedule: { kind: 'cron', expression: '* * * * *' }, nextRunMs: 60000, enabled: true, running: false,
  createdAtMs: 1000, successes: 0, failures: 0, lastRunMs: null, lastError: null, ...overrides,
})

test('only pending or currently running tasks appear in the active group', () => {
  assert.equal(scheduledTaskIsActive(task(), 5000), true)
  assert.equal(scheduledTaskState(task(), 5000), '等待执行')
  const suspended = task({ enabled: false, nextRunMs: 60000, lastError: 'Plugin disabled' })
  assert.equal(scheduledTaskIsActive(suspended, 5000), false)
  assert.equal(scheduledTaskState(suspended, 5000), '已暂停')
  // An execution can finish after its schedule expires or is suspended.
  const running = task({ running: true, enabled: false, nextRunMs: null })
  assert.equal(scheduledTaskIsActive(running, 5000), true)
  assert.equal(scheduledTaskState(running, 5000), '执行中')
})

test('once history and expired Cron tasks have distinct status', () => {
  const once = task({ schedule: { kind: 'once', atMs: 2000 }, enabled: false, nextRunMs: null, successes: 1 })
  assert.equal(scheduledTaskState(once, 5000), '已结束')
  assert.equal(scheduledTaskIsActive(once, 5000), false)
  const expired = task({ schedule: { kind: 'cron', expression: '* * * * *', endMs: 3000 } })
  assert.equal(scheduledTaskState(expired, 5000), '已到期')
  assert.equal(scheduledTaskIsActive(expired, 5000), false)
  const suspendedOnce = task({ schedule: { kind: 'once', atMs: 2000 }, enabled: false, nextRunMs: null, lastError: 'Plugin disabled' })
  assert.equal(scheduledTaskState(suspendedOnce, 5000), '已暂停')
  assert.equal(scheduledTaskState({ ...suspendedOnce, lastRunMs: 2000, failures: 1, lastError: 'Callback failed' }, 5000), '已结束')
})

test('old tasks without logs remain usable and logs are shown newest first without mutating the task', () => {
  assert.deepEqual(scheduledTaskLogs(task()), [])
  const logs = [
    { executeAtMs: 1000, durationMs: 15, success: true, error: null },
    { executeAtMs: 3000, durationMs: 25, success: false, error: 'Callback failed' },
  ]
  const result = scheduledTaskLogs(task({ logs }))
  assert.deepEqual(result.map(log => log.executeAtMs), [3000, 1000])
  assert.equal(result[0].error, 'Callback failed')
  assert.equal(result[0].durationMs, 25)
  assert.deepEqual(logs.map(log => log.executeAtMs), [1000, 3000])
})

test('rule display includes boundaries, preserves epoch zero and handles missing timestamps', () => {
  assert.equal(scheduledTaskTime(null), '—')
  assert.equal(scheduledTaskTime(undefined), '—')
  assert.notEqual(scheduledTaskTime(0), '—')
  assert.equal(scheduledTaskTime(Number.NaN), '时间无效')
  assert.match(scheduledTaskRule(task({ schedule: { kind: 'once', atMs: 0 } })), /^一次性 · /)
  const rule = scheduledTaskRule(task({ schedule: { kind: 'cron', expression: '0 9 * * MON-FRI', startMs: 0, endMs: 60000 } }))
  assert.ok(rule.includes('0 9 * * MON-FRI'))
  assert.ok(rule.includes('开始 '))
  assert.ok(rule.includes('截止 '))
})
