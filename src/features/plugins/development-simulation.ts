import { validateInvocationContext, type InvocationContext } from '../../../plugin-sdk/declarative-v1/web/invocation-context.mjs'

export type DevelopmentSimulationMode = 'manager' | 'launcher' | 'text' | 'files' | 'directories'
export interface DevelopmentSimulation {
  surface: 'manager' | 'launcher' | 'super-panel'
  context: InvocationContext | null
}

/** Produce a bounded fixture only: no clipboard, Explorer, or file IO occurs. */
export function createDevelopmentSimulation(mode: DevelopmentSimulationMode, value: string): DevelopmentSimulation {
  if (mode === 'manager') return { surface: 'manager', context: null }
  if (!['launcher', 'text', 'files', 'directories'].includes(mode)) throw new Error('请选择有效的模拟入口。')
  if (!value.trim()) throw new Error(mode === 'files' || mode === 'directories' ? '请输入至少一个绝对路径。' : '请输入要传给插件的模拟文字。')
  if (mode === 'launcher' || mode === 'text') {
    try {
      return { surface: mode === 'launcher' ? 'launcher' : 'super-panel', context: validateInvocationContext({
        type: 'text', source: mode === 'launcher' ? 'launcher-input' : 'selected-text', text: value,
      }) }
    } catch { throw new Error('模拟文字格式或长度无效；Launcher 最多 300 个字符，选中文字最多 32 KiB。') }
  }
  const paths = value.split(/\r?\n/u).map(item => item.trim()).filter(Boolean)
  if (paths.length > 64) throw new Error('模拟文件或目录最多 64 项。')
  if (new Set(paths.map(item => item.toLowerCase())).size !== paths.length) throw new Error('模拟路径不能重复。')
  try {
    return { surface: 'super-panel', context: validateInvocationContext({ type: mode, source: 'explorer-selection',
      items: paths.map(path => ({ kind: mode === 'files' ? 'file' : 'directory', path })),
    }) }
  } catch { throw new Error('模拟文件或目录格式无效；每行填写一个 Windows 绝对路径，最多 64 项。') }
}
