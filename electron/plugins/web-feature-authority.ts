import type { CapabilityV2 } from '../../plugin-sdk/declarative-v1/runtime/manifest-v2.mjs'
import { isValidatedPluginRuntimeSnapshot, type ValidatedPluginRuntimeSnapshot } from './package-validator.ts'

const authorities = new WeakSet<object>()
export type WebCapability = CapabilityV2 | 'plugin.window.manage' | 'clipboard.current.read' | 'system.file.select' | 'system.path.open' | 'system.notification' | 'system.input.paste' | 'system.screen.capture' | 'plugin.tasks' | 'system.browser.automation' | 'system.media.image' | 'system.media.transcode'
export const IMPLEMENTED_WEB_CAPABILITIES: readonly WebCapability[] = ['plugin.storage.read', 'plugin.storage.write', 'sharedAI.complete', 'clipboard.write', 'clipboard.read', 'plugin.config.read', 'plugin.config.write', 'external.open', 'plugin.window.manage', 'clipboard.current.read', 'system.file.select', 'system.path.open', 'system.notification', 'system.input.paste', 'system.screen.capture', 'plugin.tasks', 'system.browser.automation', 'system.media.image', 'system.media.transcode']
/** An implemented method is usable only if this validated package defines its inputs. */
export function isWebCapabilityImplemented(capability: WebCapability, manifest: { manifestVersion: number; configuration?: unknown }): boolean {
  return IMPLEMENTED_WEB_CAPABILITIES.includes(capability)
    && (capability !== 'external.open' || manifest.manifestVersion === 3)
    && (capability !== 'plugin.window.manage' || manifest.manifestVersion === 3)
    && (capability !== 'clipboard.current.read' || manifest.manifestVersion === 3)
    && (!capability.startsWith('system.') || manifest.manifestVersion === 3)
    && (!capability.startsWith('plugin.config.') || manifest.manifestVersion === 3 && manifest.configuration !== undefined)
}
export interface WebFeatureAuthority {
  readonly snapshot: ValidatedPluginRuntimeSnapshot
  readonly featureId: string
  readonly development?: Readonly<{ sessionId: string; originIdentity: string }>
  isCurrent(capability?: WebCapability): boolean
  revoke(): void
  onRevoke(listener: () => void): () => void
}

/** Host-internal ticket; never serializable or derived from Renderer claims. */
export function createWebFeatureAuthority(snapshot: ValidatedPluginRuntimeSnapshot, featureId: string,
  current: (capability?: WebCapability) => boolean, release: () => void,
  development?: Readonly<{ sessionId: string; originIdentity: string }>): WebFeatureAuthority {
  if (!isValidatedPluginRuntimeSnapshot(snapshot) || !snapshot.manifest.features.some(f => f.id === featureId)) throw new TypeError('Invalid Web authority')
  let revoked = false
  const listeners = new Set<() => void>()
  const authority = Object.freeze({ snapshot, featureId,
    ...(development ? { development: Object.freeze({ ...development }) } : {}),
    isCurrent: (capability?: WebCapability) => !revoked && current(capability),
    revoke: () => {
      if (revoked) return
      revoked = true; release()
      for (const listener of [...listeners]) { try { listener() } catch { /* Revocation cannot be interrupted by an observer. */ } }
      listeners.clear()
    },
    onRevoke: (listener: () => void) => {
      if (revoked) { listener(); return () => {} }
      listeners.add(listener); return () => { listeners.delete(listener) }
    },
  })
  authorities.add(authority)
  return authority
}
export function isWebFeatureAuthority(value: unknown): value is WebFeatureAuthority {
  return value !== null && typeof value === 'object' && authorities.has(value)
}
