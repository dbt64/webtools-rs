import test from 'node:test'
import assert from 'node:assert/strict'
import { parseDevelopmentInvocation } from './development-invocation.ts'
test('simulated entry validates bounded canonical text and Explorer contexts', () => {
  for (const input of [
    { surface: 'manager', context: null },
    { surface: 'launcher', context: { type:'text', source:'launcher-input', text:'test' } },
    { surface: 'super-panel', context: { type:'text', source:'selected-text', text:'test' } },
    { surface: 'super-panel', context: { type:'files', source:'explorer-selection', items:[{kind:'file',path:'C:\\test.txt'}] } },
    { surface: 'super-panel', context: { type:'directories', source:'explorer-selection', items:[{kind:'directory',path:'C:\\test'}] } },
  ]) assert.deepEqual(parseDevelopmentInvocation(input), input)
  for (const input of [
    { surface:'launcher',context:{ type:'text',source:'selected-text',text:'bad' } },
    { surface:'manager',context:{ type:'text',source:'launcher-input',text:'bad' } },
    { surface:'super-panel',context:{type:'files',source:'explorer-selection',items:[{kind:'file',path:'../bad'}]} },
    { surface:'super-panel',context:{type:'text',source:'selected-text',text:'x'.repeat(17000)} },
    { surface:'manager',context:null,pluginId:'other' },
  ]) assert.throws(() => parseDevelopmentInvocation(input))
})
