function emptyModelRow() {
  return { name: '', contextWindow: '', maxContextWindow: '', image: false, reasoning: true }
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
