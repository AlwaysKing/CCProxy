// 用量 store：聚合在主进程做（要读几十天日文件），这里只管记住区间、取回结果。
import { ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

export const useUsageStore = defineStore('usage', () => {
  const totals = ref(null)
  const byDay = ref([])
  const byProtocol = ref([])
  const byProvider = ref([])
  const byModel = ref([])
  const isLoading = ref(false)
  const error = ref(null)
  // 区间就一个「最近几天」，默认今天
  const range = ref({ days: 1 })

  async function load(nextRange) {
    if (nextRange) range.value = nextRange
    isLoading.value = true
    error.value = null
    try {
      // 过 IPC 前必须给纯数据：range 是 ref，range.value 拿到的是 Vue 的响应式代理，
      // 直接送进 invoke 会在 structured clone 阶段抛「An object could not be cloned」
      // （与配置 store 里 toPlain 挡的是同一个坑）
      const result = await window.electronAPI.getUsage({ days: range.value.days })
      if (!result.success) throw new Error(result.error)
      totals.value = result.totals
      byDay.value = result.byDay ?? []
      byProtocol.value = result.byProtocol ?? []
      byProvider.value = result.byProvider ?? []
      byModel.value = result.byModel ?? []
      return true
    } catch (e) {
      error.value = e.message
      logger.error('加载用量统计失败', { error: e.message })
      return false
    } finally {
      isLoading.value = false
    }
  }

  return {
    // State
    totals,
    byDay,
    byProtocol,
    byProvider,
    byModel,
    isLoading,
    error,
    range,
    // Actions
    load
  }
})
