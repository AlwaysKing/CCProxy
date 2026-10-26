// 项目默认模型页的唯一前端状态：自动发现、候选模型和同步状态都以主进程返回为准。
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

const EMPTY_VIEW = {
  enabled: false,
  candidates: { codex: [], claude: [] },
  projects: [],
  codexDaemon: null
}

export const useProjectModelsStore = defineStore('projectModels', () => {
  const view = ref(EMPTY_VIEW)
  const loading = ref(false)
  const busy = ref(false)
  const notice = ref('')
  const search = ref('')
  const showHidden = ref(false)

  const enabled = computed(() => view.value.enabled)
  const candidates = computed(() => view.value.candidates ?? EMPTY_VIEW.candidates)
  const daemon = computed(() => view.value.codexDaemon)
  const projects = computed(() => view.value.projects ?? [])

  const filteredProjects = computed(() => {
    const needle = search.value.trim().toLowerCase()
    return projects.value.filter((project) => {
      if (!showHidden.value && project.hidden) return false
      return !needle || project.path.toLowerCase().includes(needle)
    })
  })

  function applyView(next) {
    view.value = next ? { ...EMPTY_VIEW, ...next } : EMPTY_VIEW
  }

  async function reload() {
    loading.value = true
    try {
      const result = await window.electronAPI.getProjectModels()
      if (!result.success) throw new Error(result.error || '读取项目模型失败')
      applyView(result.view)
      notice.value = ''
    } catch (error) {
      notice.value = error.message || '读取项目模型失败'
      logger.error('读取项目模型失败', { error: error.message })
    } finally {
      loading.value = false
    }
  }

  async function refresh() {
    busy.value = true
    try {
      const result = await window.electronAPI.refreshProjectModels()
      if (!result.success) throw new Error(result.error || '刷新项目失败')
      applyView(result.view)
      notice.value = ''
    } catch (error) {
      notice.value = error.message || '刷新项目失败'
    } finally {
      busy.value = false
    }
  }

  async function setEnabled(next) {
    busy.value = true
    try {
      const result = await window.electronAPI.setProjectSyncEnabled(next)
      if (!result.success) throw new Error(result.error || '切换项目模型同步失败')
      await reload()
    } catch (error) {
      notice.value = error.message || '切换项目模型同步失败'
    } finally {
      busy.value = false
    }
  }

  async function add() {
    busy.value = true
    notice.value = ''
    try {
      const picked = await window.electronAPI.pickProjectDir()
      if (!picked.success) throw new Error(picked.error || '选择项目目录失败')
      if (!picked.path) return
      const result = await window.electronAPI.addProjectDir(picked.path)
      if (!result.success) throw new Error(result.error || '添加项目失败')
      await reload()
    } catch (error) {
      notice.value = error.message || '添加项目失败'
    } finally {
      busy.value = false
    }
  }

  async function setModel(project, client, model) {
    busy.value = true
    notice.value = ''
    try {
      const result = await window.electronAPI.setProjectModel(project.path, client, model)
      if (!result.success) throw new Error(result.error || '保存项目模型失败')
      await reload()
      if (result.status?.message) notice.value = result.status.message
      return true
    } catch (error) {
      notice.value = error.message || '保存项目模型失败'
      return false
    } finally {
      busy.value = false
    }
  }

  async function hide(project) {
    busy.value = true
    try {
      const result = project.hidden
        ? await window.electronAPI.unhideProject(project.path)
        : await window.electronAPI.hideProject(project.path)
      if (!result.success) throw new Error(result.error || '更新项目显示状态失败')
      await reload()
    } catch (error) {
      notice.value = error.message || '更新项目显示状态失败'
    } finally {
      busy.value = false
    }
  }

  async function remove(project) {
    busy.value = true
    try {
      const result = await window.electronAPI.removeProject(project.path)
      if (!result.success) throw new Error(result.error || '移除项目失败')
      await reload()
    } catch (error) {
      notice.value = error.message || '移除项目失败'
    } finally {
      busy.value = false
    }
  }

  return {
    view,
    loading,
    busy,
    notice,
    search,
    showHidden,
    enabled,
    candidates,
    daemon,
    projects,
    filteredProjects,
    reload,
    refresh,
    setEnabled,
    add,
    setModel,
    hide,
    remove
  }
})
