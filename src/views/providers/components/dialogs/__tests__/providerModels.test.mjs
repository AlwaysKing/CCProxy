import test from 'node:test'
import assert from 'node:assert/strict'

import {
  groupsForEditor,
  modelAttributesEnabled,
  modelListLabel,
  replaceFetchedModels
} from '../providerModels.mjs'

test('replaceFetchedModels replaces removed models while preserving retained metadata', () => {
  const existing = [
    { name: 'retired-model', contextWindow: '64000', image: false, reasoning: false },
    { name: 'kept-model', contextWindow: '128000', maxContextWindow: '256000', image: true, reasoning: true }
  ]

  const next = replaceFetchedModels(existing, [' kept-model ', 'new-model', 'new-model', ''])

  assert.deepEqual(next, [
    { name: 'kept-model', contextWindow: '128000', maxContextWindow: '256000', image: true, reasoning: true },
    { name: 'new-model', contextWindow: '', maxContextWindow: '', image: false, reasoning: true }
  ])
})

test('groupsForEditor keeps OpenAI Chat and Responses model lists separate', () => {
  const groups = groupsForEditor([
    { id: 'g_anthropic', apiFormat: 'anthropic', models: [{ name: 'claude' }] },
    { id: 'g_openai_chat', apiFormat: 'openai_chat', models: [{ name: 'chat-model' }] },
    { id: 'g_openai_responses', apiFormat: 'openai_responses', models: [{ name: 'responses-model' }] }
  ])

  assert.deepEqual(
    groups.map((group) => [group.apiFormat, group.models.map((model) => model.name)]),
    [
      ['anthropic', ['claude']],
      ['openai_chat', ['chat-model']],
      ['openai_responses', ['responses-model']]
    ]
  )
})

test('groupsForEditor expands a legacy combined OpenAI group into both protocols', () => {
  const groups = groupsForEditor([
    { id: 'g_openai', apiFormat: 'openai', models: [{ name: 'shared-model' }] }
  ])

  const openai = groups.filter((group) => group.apiFormat.startsWith('openai'))
  assert.deepEqual(
    openai.map((group) => [group.id, group.apiFormat, group.models.map((model) => model.name)]),
    [
      ['g_openai_chat', 'openai_chat', ['shared-model']],
      ['g_openai_responses', 'openai_responses', ['shared-model']]
    ]
  )
})

test('only OpenAI Responses exposes model capability fields', () => {
  assert.equal(modelAttributesEnabled('anthropic'), false)
  assert.equal(modelAttributesEnabled('openai_chat'), false)
  assert.equal(modelAttributesEnabled('openai_responses'), true)
})

test('OpenAI Responses model list label does not mention Codex', () => {
  const label = modelListLabel('openai_responses')

  assert.equal(label, 'OpenAI Responses 模型列表')
  assert.equal(label.includes('Codex'), false)
})
