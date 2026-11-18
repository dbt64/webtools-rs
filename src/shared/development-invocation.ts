import { validateInvocationContext, type InvocationContext } from '../../plugin-sdk/declarative-v1/web/invocation-context.mjs'
export interface DevelopmentInvocation { surface: 'manager' | 'launcher' | 'super-panel'; context: InvocationContext | null }
export function parseDevelopmentInvocation(value: unknown): DevelopmentInvocation {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid development invocation')
  const v = value as Record<string, unknown>
  if (Object.keys(v).length !== 2 || !Object.hasOwn(v, 'surface') || !Object.hasOwn(v, 'context')
    || !['manager', 'launcher', 'super-panel'].includes(String(v.surface))
    || new TextEncoder().encode(JSON.stringify(value)).length > 16 * 1024) throw new Error('Invalid development invocation')
  const context = validateInvocationContext(v.context)
  if (v.context !== null && !context) throw new Error('Invalid development context')
  if (v.surface === 'manager' && context !== null
    || v.surface === 'launcher' && context !== null && (context.type !== 'text' || context.source !== 'launcher-input')
    || v.surface === 'super-panel' && context !== null && context.source !== 'selected-text' && context.source !== 'explorer-selection') throw new Error('Context does not match simulated entry')
  return { surface: v.surface as DevelopmentInvocation['surface'], context }
}
