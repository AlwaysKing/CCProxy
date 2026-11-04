import test from 'node:test'
import assert from 'node:assert/strict'

import {
  modelOptionsForProtocol,
  modelSelectGroupsForProtocol,
  upstreamLabel
} from '../mappingModels.mjs'

const provider = {
  id: 'p1',
  name: '供应商',
  modelGroups: [
    { id: 'g_chat', apiFormat: 'openai_chat', models: [{ name: 'chat-only' }] },
    { id: 'g_responses', apiFormat: 'openai_responses', models: [{ name: 'responses-only' }] }
  ]
}

test('real model options only contain models from the selected upstream protocol', () => {
  assert.deepEqual(
    modelOptionsForProtocol(provider, 'openai_chat').map((option) => option.value),
    ['chat-only']
  )
  assert.deepEqual(
    modelOptionsForProtocol(provider, 'openai_responses').map((option) => option.value),
    ['responses-only']
  )
})

test('real model select labels the active upstream model group', () => {
  const groups = modelSelectGroupsForProtocol(provider, 'openai_responses')

  assert.deepEqual(groups, [
    {
      key: 'g_responses',
      label: 'OpenAI Responses 模型组（1）',
      options: [{ value: 'responses-only', label: 'responses-only' }]
    }
  ])
})

test('upstream labels keep OpenAI protocols distinct', () => {
  assert.equal(upstreamLabel('openai_chat'), 'OpenAI Chat')
  assert.equal(upstreamLabel('openai_responses'), 'OpenAI Responses')
})
