// 统一项目管理页的唯一前端状态：项目发现、候选模型和同步状态都以主进程返回为准。
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

const EMPTY_VIEW = {
  candidates: { codex: [], claude: [] },
  projects: [],
  codexDaemon: null
}

export const useProjectModelsStore = defineStore('projectModels', () => {
  const view = ref(EMPTY_VIEW)
  const loading = ref(false)
  const notice = ref('')
  const search = ref('')
  const pendingKeys = ref(new Set())

  const candidates = computed(() => view.value.candidates ?? EMPTY_VIEW.candidates)
  const projects = computed(() => view.value.projects ?? [])
  const filteredProjects = computed(() => {
    const needle = search.value.trim().toLowerCase()
    return projects.value.filter(
      (project) => !needle || project.path.toLowerCase().includes(needle)
    )
  })

  function applyView(next) {
    view.value = next ? { ...EMPTY_VIEW, ...next } : EMPTY_VIEW
  }

  function isPending(path, client) {
    return pendingKeys.value.has(`${path}\0${client}`)
  }

  function setPending(path, client, pending) {
    const next = new Set(pendingKeys.value)
    const key = `${path}\0${client}`
    if (pending) next.add(key)
    else next.delete(key)
    pendingKeys.value = next
  }

  async function reload(refresh = false) {
    loading.value = true
    try {
      const result = refresh
        ? await window.electronAPI.refreshProjectModels()
        : await window.electronAPI.getProjectModels()
      if (!result.success) throw new Error(result.error || '读取项目失败')
      applyView(result.view)
      notice.value = ''
      return true
    } catch (error) {
      notice.value = error.message || '读取项目失败'
      logger.error('读取项目失败', { error: error.message })
      return false
    } finally {
      loading.value = false
    }
  }

  async function setModel(project, client, mappingId) {
    if (isPending(project.path, client)) return false
    setPending(project.path, client, true)
    notice.value = ''
    try {
      const result = await window.electronAPI.setProjectModel(
        project.path,
        client,
        mappingId || null
      )
      if (!result.success) throw new Error(result.error || '保存项目模型失败')
      await reload()
      if (result.status?.message) notice.value = result.status.message
      return true
    } catch (error) {
      const message = error.message || '保存项目模型失败'
      await reload()
      notice.value = message
      return false
    } finally {
      setPending(project.path, client, false)
    }
  }

  return {
    view,
    loading,
    notice,
    search,
    candidates,
    projects,
    filteredProjects,
    reload,
    setModel,
    isPending
  }
})
