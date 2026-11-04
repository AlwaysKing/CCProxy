import test from 'node:test'
import assert from 'node:assert/strict'

import { replaceFetchedModels } from '../providerModels.mjs'

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
