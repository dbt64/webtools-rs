import assert from 'node:assert/strict'
import test from 'node:test'
import { FeatureActivationService, contextForFeature } from './feature-activation.ts'

const ref = { pluginId: 'org.example.quick', featureId: 'quick-text' }
test('shortcut opens window without incompatible selection while commands and panel remain constrained', () => {
  const feature = { acceptedContexts: [{ type: 'files', sources: ['explorer-selection'], extensions: ['.png'] }], presentation: { type: 'window' } }
  const text = { type: 'text', source: 'selected-text', text: 'hello', ageMs: 0 }
  const image = { type: 'files', source: 'explorer-selection', items: [{ kind: 'file', path: 'C:\\image.png' }], ageMs: 0 }
  assert.deepEqual(contextForFeature(feature, image, 'hotkey'), image)
  const textFeature = { acceptedContexts: [{ type: 'text', sources: ['selected-text'] }], presentation: { type: 'window' } }
  assert.deepEqual(contextForFeature(textFeature, text, 'hotkey'), text)
  const directoryFeature = { acceptedContexts: [{ type: 'directories', sources: ['explorer-selection'] }], presentation: { type: 'window' } }
  const directory = { type: 'directories', source: 'explorer-selection', items: [{ kind: 'directory', path: 'C:\\sample' }], ageMs: 0 }
  assert.deepEqual(contextForFeature(directoryFeature, directory, 'hotkey'), directory)
  assert.equal(contextForFeature(feature, text, 'hotkey'), undefined)
  assert.throws(() => contextForFeature(feature, text, 'super-panel'), error => error.code === 'FEATURE_UNAVAILABLE')
  assert.throws(() => contextForFeature({ ...feature, presentation: { type: 'command' } }, text, 'hotkey'), error => error.code === 'FEATURE_UNAVAILABLE')
})
test('Quick Result completion checks live source independently of released runtime authority', async () => {
  for (const valid of [true, false]) {
    let released = false
    const service = new FeatureActivationService(async (_ref, dispatch) => ({ completion: dispatch({ isCurrent: () => !released }, () => valid) }),
      { present: async () => { released = true; return { status: 'completed', result: { kind: 'text', text: 'computed' } } } })
    const request = { ref, surface: 'super-panel', quickResult: { resultTarget: 'super-panel', signal: new AbortController().signal } }
    if (valid) assert.equal((await service.activate(request)).result.text, 'computed')
    else await assert.rejects(service.activate(request), error => error.code === 'FEATURE_UNAVAILABLE')
  }
})
