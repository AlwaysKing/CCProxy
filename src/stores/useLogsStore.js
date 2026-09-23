// 请求日志 store：主进程留最近 500 条，这里持有同一份的本地拷贝。
// 进页面拉一次全量，之后靠 log-appended 推送往前插——不轮询：
// 代理一旦跑起来请求是连续的，轮询只是白白来回 IPC
import { ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

// 与主进程 request-log 的 MAX_ENTRIES 同值。两边不一致会出现「界面显示 300 条、
// 实际留了 500 条」这种对不上的观感，这个数没有别的地方用，就近在这儿再写一份
const MAX_ENTRIES = 500

export const useLogsStore = defineStore('logs', () => {
  const entries = ref([])
  const isLoading = ref(false)
  const error = ref(null)

  async function init() {
    isLoading.value = true
    error.value = null
    try {
      const result = await window.electronAPI.getLogs()
      if (!result.success) throw new Error(result.error)
      entries.value = result.logs ?? []
    } catch (e) {
      error.value = e.message
      logger.error('加载请求日志失败', { error: e.message })
    } finally {
      isLoading.value = false
    }
  }

  // 推送来的条目插到最前并裁到上限：列表增长不该等下一次整表刷新
  function applyEntry(entry) {
    if (!entry) return
    entries.value.unshift(entry)
    if (entries.value.length > MAX_ENTRIES) entries.value.length = MAX_ENTRIES
  }

  async function clear() {
    isLoading.value = true
    error.value = null
    try {
      const result = await window.electronAPI.clearLogs()
      if (!result.success) throw new Error(result.error)
      entries.value = []
      return true
    } catch (e) {
      error.value = e.message
      logger.error('清空请求日志失败', { error: e.message })
      return false
    } finally {
      isLoading.value = false
    }
  }

  return {
    // State
    entries,
    isLoading,
    error,
    // Actions
    init,
    applyEntry,
    clear
  }
})
