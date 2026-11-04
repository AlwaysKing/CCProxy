export function upstreamLabel(protocol) {
  if (protocol === 'anthropic') return 'Claude / Anthropic'
  if (protocol === 'openai_chat') return 'OpenAI Chat'
  if (protocol === 'openai_responses') return 'OpenAI Responses'
  return ''
}

export function modelGroupForProtocol(provider, upstreamProtocol) {
  return provider?.modelGroups.find((group) => group.apiFormat === upstreamProtocol) || null
}

export function modelOptionsForProtocol(provider, upstreamProtocol, currentModel = '') {
  const group = modelGroupForProtocol(provider, upstreamProtocol)
  const names = (group?.models ?? []).map((model) => model.name)
  const options = names.map((name) => ({ value: name, label: name }))
  if (currentModel && !names.includes(currentModel)) {
    options.unshift({
      value: currentModel,
      label: `${currentModel}（不在 ${upstreamLabel(upstreamProtocol)} 模型组中）`
    })
  }
  return options
}

export function modelSelectGroupsForProtocol(provider, upstreamProtocol, currentModel = '') {
  const group = modelGroupForProtocol(provider, upstreamProtocol)
  if (!group) return []
  return [
    {
      key: group.id || upstreamProtocol,
      label: `${upstreamLabel(upstreamProtocol)} 模型组（${group.models?.length ?? 0}）`,
      options: modelOptionsForProtocol(provider, upstreamProtocol, currentModel)
    }
  ]
}
