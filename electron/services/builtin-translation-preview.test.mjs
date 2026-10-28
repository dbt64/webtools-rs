import test from 'node:test'
import assert from 'node:assert/strict'
import { BuiltinTranslationPreview } from './builtin-translation-preview.ts'
import { ServiceError } from './service-error.ts'
import { createDefaultTranslation2WorkspaceSettings } from '../../src/shared/translation2-contracts.ts'

const id = '6528d127-0182-4a60-9e4b-8c89070c50f0'
const MYMEMORY_TARGET = { kind: 'machine', adapterId: 'mymemory', instanceId: 'builtin.mymemory' }

function fixture({ translate, translation2, settingsMutator, enabled: enabledCheck } = {}) {
  const settings = {
    translation: { engine: 'mymemory', sourceLanguage: 'auto', targetLanguage: 'zh-CN', qwenMtModel: 'qwen-mt-flash' },
    translation2: { ...createDefaultTranslation2WorkspaceSettings(), ...translation2 },
    sharedAI: { defaultProviderId: 'custom', providers: { custom: { model: 'stub' }, qwen: { model: 'qwen-mt-flash', region: 'cn-beijing', workspaceId: 'stub' } } },
  }
  settingsMutator?.(settings)
  const requests = []
  const cancels = []
  let enabled = true
  let admission = 1
  let rejectPending = null
  const service = {
    captureAdmissionGeneration: () => admission,
    isAdmissionGenerationCurrent: value => value === admission,
    cancel: requestId => {
      cancels.push(requestId)
      rejectPending?.(new ServiceError('TRANSLATION_CANCELLED', 'cancelled'))
    },
    translateWithTarget: async (request, target) => {
      requests.push({ request, target })
      const translation = translate
        ? await translate(request.signal)
        : await new Promise((resolve, reject) => {
          rejectPending = reject
          resolve('isolated translated text')
        })
      return { translation, provider: { engine: 'mymemory', providerName: 'Stub Provider', configured: true } }
    },
  }
  const preview = new BuiltinTranslationPreview({
    service,
    settings: () => structuredClone(settings),
    enabled: enabledCheck ?? (async () => enabled),
  })
  return {
    settings, requests, cancels, preview,
    disable() { enabled = false; preview.invalidate() },
    revoke() { admission++; preview.invalidate() },
  }
}

test('preview forwards the configured Translation 2.0 target with an English auto source', async () => {
  const machine = fixture({ translate: async () => 'translated' })
  const value = await machine.preview.run(id, 'capability', new AbortController().signal)
  assert.match(value.text, /translated/)
  assert.equal(value.isCurrent(), true)
  assert.equal(machine.requests.length, 1)
  assert.deepEqual(machine.requests[0].target, MYMEMORY_TARGET)
  assert.equal(machine.requests[0].request.sourceLanguage, 'en')
  assert.equal(machine.requests[0].request.targetLanguage, 'zh-CN')
  assert.equal(JSON.stringify(machine.preview).includes('capability'), false)

  const ai = fixture({
    translate: async () => 'ai translated',
    translation2: { selectedService: { kind: 'ai', providerId: 'custom' } },
  })
  await ai.preview.run(id, 'English text', new AbortController().signal)
  assert.deepEqual(ai.requests[0].target, { kind: 'ai', providerId: 'custom' })

  const deepl = fixture({
    translate: async () => 'deepl translated',
    translation2: {
      selectedService: { kind: 'machine', instanceId: 'machine.deepl' },
      machineServices: [{
        id: 'machine.deepl', name: 'DeepL', adapterId: 'deepl', enabled: true, builtIn: false, testStatus: 'passed',
      }],
    },
  })
  await deepl.preview.run(id, 'English text', new AbortController().signal)
  assert.deepEqual(deepl.requests[0].target, { kind: 'machine', adapterId: 'deepl', instanceId: 'machine.deepl' })
})

test('disabled setting, non-English content and provider input limits cause zero provider requests', async () => {
  const disabled = fixture({ translate: async () => 'unused' })
  disabled.disable()
  assert.equal(await disabled.preview.run(id, 'hello', new AbortController().signal), null)
  const env = fixture({ translate: async () => 'unused' })
  for (const text of ['', '中文', 'https://example.com', 'hello 中文', 'a'.repeat(3001)]) {
    assert.equal(await env.preview.run(id, text, new AbortController().signal), null)
  }
  assert.match((await env.preview.run(id, 'a'.repeat(501), new AbortController().signal)).text, /500|过长|限制/)
  env.settings.translation.sourceLanguage = 'ja'
  assert.match((await env.preview.run(id, 'hello', new AbortController().signal)).text, /来源|英语/)
  assert.equal(env.requests.length, 0)
})

test('an unconfigured provider reports setup state without a request or fallback', async () => {
  const env = fixture({ translate: async () => { throw new ServiceError('AI_NOT_CONFIGURED', 'PRIVATE_PROVIDER_DETAIL') } })
  const value = await env.preview.run(id, 'hello world', new AbortController().signal)
  assert.match(value.text, /尚未配置/)
  assert.equal(value.text.includes('PRIVATE_PROVIDER_DETAIL'), false)
})

test('cancel, setting change, new preview and admission revoke abort provider work and suppress late output', async () => {
  for (const action of ['cancel', 'settings', 'replace', 'revoke']) {
    let resolve
    let started
    const gate = new Promise(done => { started = done })
    const env = fixture({
      translate: () => { started(); return new Promise(done => { resolve = done }) },
    })
    const abort = new AbortController()
    const pending = env.preview.run(id, 'first English selection', abort.signal)
    await gate
    if (action === 'cancel') abort.abort()
    if (action === 'settings') env.settings.translation.targetLanguage = 'ja'
    if (action === 'revoke') env.revoke()
    if (action === 'replace') await env.preview.run('eec8790c-1b26-428e-b41f-33acdb9d3fb5', '中文', new AbortController().signal)
    resolve('LATE_PRIVATE_OUTPUT')
    assert.equal(await pending, null)
    if (action !== 'settings') {
      assert.equal(env.cancels[0], `panel-${id}`)
      assert.equal(env.cancels.length, 1)
    }
  }
})

test('provider errors and oversized output use safe bounded product copy, never raw provider messages', async () => {
  for (const output of [
    () => { throw new ServiceError('FREE_TRANSLATION_QUOTA', 'PRIVATE_TOKEN_AND_TEXT') },
    () => { throw new Error('PRIVATE_TOKEN_AND_TEXT') },
    () => 'x'.repeat(8193),
    () => ' \n',
  ]) {
    const env = fixture({ translate: async () => output() })
    const value = await env.preview.run(id, 'private English selection', new AbortController().signal)
    assert.equal(value.text.includes('PRIVATE_TOKEN_AND_TEXT'), false)
    assert.equal(value.text.includes('private English selection'), false)
    assert.ok(value.text.length > 0 && value.text.length < 500)
  }
  const empty = await fixture({ translate: async () => ' \n' }).preview.run(id, 'English text', new AbortController().signal)
  assert.match(empty.text, /为空|限制/)
})

test('a result accepted during a configuration change is no longer current', async () => {
  const env = fixture({ translate: async () => 'translated' })
  const result = await env.preview.run(id, 'English text', new AbortController().signal)
  env.settings.translation.targetLanguage = 'ja'
  assert.equal(result.isCurrent(), false)
})

test('a configuration change while the enabled check is pending prevents any provider request', async () => {
  let entered
  let resolveEnabled
  const gate = new Promise(done => { entered = done })
  const env = fixture({
    translate: async () => 'unused',
    enabled: () => { entered(); return new Promise(done => { resolveEnabled = done }) },
  })
  const pending = env.preview.run(id, 'English text', new AbortController().signal)
  await gate
  env.settings.translation.targetLanguage = 'ja'
  resolveEnabled(true)
  assert.equal(await pending, null)
  assert.equal(env.requests.length, 0)
})
