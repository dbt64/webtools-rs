import assert from 'node:assert/strict'
import test from 'node:test'
import { createDevelopmentSimulation } from './development-simulation.ts'

test('simulation distinguishes Manager, Launcher and selected-text entry context', () => {
  assert.deepEqual(createDevelopmentSimulation('manager', 'ignored'), { surface: 'manager', context: null })
  assert.deepEqual(createDevelopmentSimulation('launcher', '查找文本'), { surface: 'launcher', context: { type: 'text', source: 'launcher-input', text: '查找文本' } })
  assert.deepEqual(createDevelopmentSimulation('text', '选中文本'), { surface: 'super-panel', context: { type: 'text', source: 'selected-text', text: '选中文本' } })
})

test('file and directory simulations normalize newline lists without reading local files', () => {
  assert.deepEqual(createDevelopmentSimulation('files', 'C:\\demo\\one.txt\r\n\nD:\\demo\\two.png').context, {
    type: 'files', source: 'explorer-selection', items: [{ kind: 'file', path: 'C:\\demo\\one.txt' }, { kind: 'file', path: 'D:\\demo\\two.png' }],
  })
  assert.deepEqual(createDevelopmentSimulation('directories', '\\\\server\\share\\demo').context, {
    type: 'directories', source: 'explorer-selection', items: [{ kind: 'directory', path: '\\\\server\\share\\demo' }],
  })
})

test('invalid and oversized simulation input is refused before invocation', () => {
  assert.throws(() => createDevelopmentSimulation('unknown', 'x'))
  assert.throws(() => createDevelopmentSimulation('text', '  '), /输入/)
  assert.throws(() => createDevelopmentSimulation('launcher', 'a'.repeat(301)), /格式|长度/)
  assert.throws(() => createDevelopmentSimulation('files', 'relative.txt'), /绝对路径/)
  assert.throws(() => createDevelopmentSimulation('files', 'C:\\one.txt\nc:\\ONE.txt'), /重复/)
  assert.throws(() => createDevelopmentSimulation('files', Array.from({ length: 65 }, (_, i) => `C:\\file-${i}.txt`).join('\n')), /64/)
})
