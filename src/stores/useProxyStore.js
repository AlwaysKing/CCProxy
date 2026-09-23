// 代理状态 store：状态由主进程推来（proxy-status-changed），这里只做展示与转发操作
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

export const useProxyStore = defineStore('proxy', () => {
  const status = ref('stopped')
  const port = ref(null)
  const error = ref(null)
  const busy = ref(false)

  const isRunning = computed(() => status.value === 'running')

  function apply(next) {
    if (!next) return
    status.value = next.status ?? 'stopped'
    port.value = next.port ?? null
    error.value = next.error ?? null
  }

  async function init() {
    try {
      const result = await window.electronAPI.getProxyStatus()
      if (result.success) apply(result)
    } catch (e) {
      logger.error('读取代理状态失败', { error: e.message })
    }
  }

  async function run(action) {
    busy.value = true
    try {
      const result = await action()
      if (result.status) apply(result)
      if (!result.success) error.value = result.error || '操作失败'
      else error.value = null
      return !!result.success
    } catch (e) {
      error.value = e.message
      logger.error('代理操作失败', { error: e.message })
      return false
    } finally {
      busy.value = false
    }
  }

  const start = () => run(() => window.electronAPI.startProxy())
  const stop = () => run(() => window.electronAPI.stopProxy())
  const restart = () => run(() => window.electronAPI.restartProxy())

  return {
    // State
    status,
    port,
    error,
    busy,
    // Computed
    isRunning,
    // Actions
    init,
    apply,
    start,
    stop,
    restart
  }
})
