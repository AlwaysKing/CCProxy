// 配置 store：唯一真源在主进程，本 store 只持有最近一次快照，
// 任何改动都先落本地副本再整体落盘，用主进程返回的归一化结果回填。
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

export const useConfigStore = defineStore('config', () => {
  const config = ref(null)
  const issues = ref([])
  const recovered = ref(false)
  const backupPath = ref('')
  const configPath = ref('')
  const isLoading = ref(false)
  const saving = ref(false)
  const error = ref(null)

  const protocols = computed(() => config.value?.protocols ?? [])
  // 供应商是全局池，三个协议共用；协议只持有映射表
  const providers = computed(() => config.value?.providers ?? [])

  function findProtocol(draft, protocolId) {
    return draft.protocols.find((item) => item.id === protocolId)
  }

  // 配置本身是 JSON（要落成 app.json），过 IPC 之前必须转成纯数据：
  // 直接送 Vue 的响应式代理会在 structured clone 阶段失败，报「An object could not be cloned」
  function toPlain(value) {
    return JSON.parse(JSON.stringify(value))
  }

  function applyState(state) {
    if (!state) return
    if (state.config) config.value = state.config
    if (state.issues) issues.value = state.issues
    if (typeof state.recovered === 'boolean') recovered.value = state.recovered
    if (typeof state.backupPath === 'string') backupPath.value = state.backupPath
    if (state.configPath) configPath.value = state.configPath
  }

  async function init() {
    isLoading.value = true
    error.value = null
    try {
      const result = await window.electronAPI.getAppState()
      if (!result.success) throw new Error(result.error)
      applyState(result)
    } catch (e) {
      error.value = e.message
      logger.error('加载配置失败', { error: e.message })
    } finally {
      isLoading.value = false
    }
  }

  async function save() {
    saving.value = true
    error.value = null
    try {
      const result = await window.electronAPI.saveConfig(toPlain(config.value))
      if (!result.success) throw new Error(result.error)
      config.value = result.config
      issues.value = result.issues ?? []
      return true
    } catch (e) {
      error.value = e.message
      logger.error('保存配置失败', { error: e.message })
      return false
    } finally {
      saving.value = false
    }
  }

  // 所有改动都先改本地副本，立刻整体落盘；主进程归一化后的结果再回填，避免两边状态漂移
  async function mutate(recipe) {
    if (!config.value) return false
    const draft = toPlain(config.value)
    recipe(draft)
    config.value = draft
    return save()
  }

  function toggleProtocol(protocolId) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (protocol) protocol.enabled = !protocol.enabled
    })
  }

  function toggleProvider(providerId) {
    return mutate((draft) => {
      const provider = (draft.providers ?? []).find((item) => item.id === providerId)
      if (provider) provider.enabled = !provider.enabled
    })
  }

  function toggleMapping(protocolId, mappingId) {
    return mutate((draft) => {
      const mapping = findProtocol(draft, protocolId)?.mappings.find((item) => item.id === mappingId)
      if (mapping) mapping.enabled = !mapping.enabled
    })
  }

  function upsertProvider(provider) {
    return mutate((draft) => {
      if (!Array.isArray(draft.providers)) draft.providers = []
      const index = draft.providers.findIndex((item) => item.id === provider.id)
      if (index >= 0) draft.providers[index] = provider
      else draft.providers.push(provider)
    })
  }

  // 删供应商要连带删掉指向它的映射，不留悬空引用。供应商是全局的，
  // 所以三个协议的映射表都要清一遍
  function removeProvider(providerId) {
    return mutate((draft) => {
      draft.providers = (draft.providers ?? []).filter((item) => item.id !== providerId)
      for (const protocol of draft.protocols ?? []) {
        protocol.mappings = (protocol.mappings ?? []).filter((mapping) => mapping.providerId !== providerId)
      }
    })
  }

  // 某个供应商被哪些协议的映射引用：删它之前要说清影响范围
  function mappingsUsingProvider(providerId) {
    const hits = []
    for (const protocol of protocols.value) {
      for (const mapping of protocol.mappings ?? []) {
        if (mapping.providerId === providerId) hits.push({ protocolId: protocol.id, protocolName: protocol.name, mapping })
      }
    }
    return hits
  }

  function upsertMapping(protocolId, mapping) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      const index = protocol.mappings.findIndex((item) => item.id === mapping.id)
      if (index >= 0) protocol.mappings[index] = mapping
      else protocol.mappings.push(mapping)
    })
  }

  // 映射表顺序即匹配优先级：拖拽结束后把条目移到目标位置，整体落盘
  function reorderMapping(protocolId, mappingId, targetIndex) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      const fromIndex = protocol.mappings.findIndex((item) => item.id === mappingId)
      if (fromIndex < 0) return
      const [mapping] = protocol.mappings.splice(fromIndex, 1)
      const toIndex = Math.max(0, Math.min(targetIndex, protocol.mappings.length))
      protocol.mappings.splice(toIndex, 0, mapping)
    })
  }

  function removeMapping(protocolId, mappingId) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      protocol.mappings = protocol.mappings.filter((item) => item.id !== mappingId)
    })
  }

  function updateSetting(key, value) {
    return mutate((draft) => {
      draft.settings[key] = value
    })
  }

  return {
    // State
    config,
    issues,
    recovered,
    backupPath,
    configPath,
    isLoading,
    saving,
    error,
    // Computed
    protocols,
    providers,
    // Actions
    init,
    applyState,
    save,
    mutate,
    toggleProtocol,
    toggleProvider,
    toggleMapping,
    upsertProvider,
    removeProvider,
    upsertMapping,
    reorderMapping,
    removeMapping,
    mappingsUsingProvider,
    updateSetting
  }
})
