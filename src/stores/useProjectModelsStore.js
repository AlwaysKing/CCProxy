// 按协议隔离的项目模型子窗口状态。
import { ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

const EMPTY_VIEW = {
  protocol: '',
  client: '',
  mappingId: '',
  model: '',
  title: '项目默认模型',
  projects: [],
  directoryCandidates: [],
  codexDaemon: null
}

export const useProjectModelsStore = defineStore('projectModels', () => {
  const view = ref(EMPTY_VIEW)
  const loading = ref(false)
  const busy = ref(false)
  const notice = ref('')

  function applyView(next) {
    view.value = next ? { ...EMPTY_VIEW, ...next } : EMPTY_VIEW
  }

  async function reload(protocol, mappingId, refresh = false) {
    loading.value = true
    try {
      const result = refresh
        ? await window.electronAPI.refreshProjectModels(protocol, mappingId)
        : await window.electronAPI.getProjectModels(protocol, mappingId)
      if (!result.success) throw new Error(result.error || '读取项目模型失败')
      applyView(result.view)
      notice.value = ''
    } catch (error) {
      notice.value = error.message || '读取项目模型失败'
      logger.error('读取项目模型失败', { protocol, error: error.message })
    } finally {
      loading.value = false
    }
  }

  async function add(protocol, mappingId, paths) {
    busy.value = true
    try {
      const result = await window.electronAPI.addProjectBindings(protocol, mappingId, paths)
      if (!result.success) throw new Error(result.error || '添加项目失败')
      await reload(protocol, mappingId)
      return true
    } catch (error) {
      notice.value = error.message || '添加项目失败'
      return false
    } finally {
      busy.value = false
    }
  }

  async function remove(protocol, mappingId, id) {
    busy.value = true
    try {
      const result = await window.electronAPI.removeProjectBinding(protocol, id)
      if (!result.success) throw new Error(result.error || '移除项目失败')
      await reload(protocol, mappingId)
      return true
    } catch (error) {
      notice.value = error.message || '移除项目失败'
      return false
    } finally {
      busy.value = false
    }
  }

  return {
    view,
    loading,
    busy,
    notice,
    reload,
    add,
    remove
  }
})
