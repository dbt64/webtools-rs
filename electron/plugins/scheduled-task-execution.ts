import type { WebFeatureAuthority } from './web-feature-authority.ts'

type Authorize = (dispatch: (authority: WebFeatureAuthority) => Promise<void>) => Promise<{ completion: Promise<void> }>
const expired = () => Object.assign(new Error('Scheduled execution exceeded its 60-second authorization/execution lifetime'), { code: 'SCHEDULE_TIMEOUT' })

/** Whole-trigger deadline, including the PluginManager authorization queue.
 * Rust keeps its running owner for 90s; the 60s deadline leaves room for the
 * existing bounded Web/Node teardown. Never release a same-code lock while an
 * active execution still has side effects; expired queued tickets cannot run.
 */
export class ScheduledTaskExecution {
  private readonly running = new Set<string>()
  constructor(private readonly lifetimeMs = 60_000) {}
  async execute(key: string, authorize: Authorize, dispatch: (authority: WebFeatureAuthority, signal: AbortSignal) => Promise<void>): Promise<void> {
    if (this.running.has(key)) throw new Error('Scheduled task is already running')
    this.running.add(key)
    const controller = new AbortController()
    let current: WebFeatureAuthority | undefined
    let active: Promise<void> | undefined
    let timer: ReturnType<typeof setTimeout> | undefined
    const deadline = new Promise<never>((_, reject) => {
      timer = setTimeout(() => { controller.abort(); current?.revoke(); reject(expired()) }, this.lifetimeMs)
    })
    const work = Promise.resolve().then(() => authorize(authority => {
      // A queued authorization may complete after execute() already timed out.
      // It must revoke its newly-issued ticket without creating any runtime.
      if (controller.signal.aborted) { authority.revoke(); return Promise.reject(expired()) }
      current = authority
      active = Promise.resolve().then(() => {
        if (controller.signal.aborted) throw expired()
        return dispatch(authority, controller.signal)
      }).finally(() => { authority.revoke(); if (current === authority) current = undefined })
      return active
    })).then(ticket => ticket.completion)
    // Keep consuming a late queued ticket's rejection after the deadline wins.
    void work.catch(() => {})
    try { await Promise.race([work, deadline]) }
    catch (error) {
      controller.abort(); current?.revoke()
      // Dispatch implements bounded owned-runtime teardown on abort. Keep both
      // this lock and the Rust caller pending until that teardown is complete.
      await active?.catch(() => {})
      throw error
    } finally { clearTimeout(timer); this.running.delete(key) }
  }
}
