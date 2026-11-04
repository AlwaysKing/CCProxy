function emptyModelRow() {
  return { name: '', contextWindow: '', maxContextWindow: '', image: false, reasoning: true }
}

const EDITOR_FORMATS = ['anthropic', 'openai_chat', 'openai_responses']

export function modelAttributesEnabled(apiFormat) {
  return apiFormat === 'openai_responses'
}

export function modelListLabel(apiFormat) {
  if (apiFormat === 'anthropic') return 'Claude 模型列表'
  if (apiFormat === 'openai_chat') return 'OpenAI Chat 模型列表'
  if (apiFormat === 'openai_responses') return 'OpenAI Responses 模型列表'
  return `${apiFormat} 模型列表`
}

export function groupsForEditor(groups) {
  const existing = new Map()
  for (const group of groups ?? []) {
    if (group?.apiFormat) existing.set(group.apiFormat, group)
  }
  const legacyOpenAI = existing.get('openai')
  return EDITOR_FORMATS.map((apiFormat) => {
    const source =
      existing.get(apiFormat) ||
      (apiFormat === 'anthropic' ? null : legacyOpenAI)
    return {
      id: source && source.apiFormat === apiFormat ? source.id : `g_${apiFormat}`,
      apiFormat,
      models: source?.models ?? []
    }
  })
}

export function replaceFetchedModels(existingRows, fetchedNames) {
  const existingByName = new Map()
  for (const row of existingRows ?? []) {
    const name = String(row?.name ?? '').trim()
    if (name && !existingByName.has(name)) {
      existingByName.set(name, row)
    }
  }

  const seen = new Set()
  const models = []
  for (const candidate of fetchedNames ?? []) {
    const name = String(candidate ?? '').trim()
    if (!name || seen.has(name)) continue
    seen.add(name)
    const existing = existingByName.get(name)
    models.push(existing ? { ...existing, name } : { ...emptyModelRow(), name })
  }
  return models
}
