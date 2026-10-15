// 客户端配置目录 store：扫描 + 检查结果只在这一份，顶栏的提示徽标和设置页的
// 目录列表吃同一个状态——在设置页修好问题，顶栏的提示跟着消失。
// 列表与检查都由主进程算（get-client-dirs 一次拿全），这里不做本地推断
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

export const useClientDirsStore = defineStore('clientDirs', () => {
  const builtin = ref([])
  const manual = ref([])
  const expectedAddress = ref('')
  // codex 的 app-server daemon 体检结果（没有 codex 目录时为 null）
  const daemon = ref(null)
  const notice = ref('')
  const loaded = ref(false)
  const busy = ref(false)

  // 「有问题」= 检查不是 ok（含读不出来的）。顶栏徽标只关心数量，
  // 具体差在哪在设置页每一行的悬停提示里
  const problemCount = computed(() =>
    [...builtin.value, ...manual.value].filter((entry) => entry.status !== 'ok').length
  )

  function applyResult(result) {
    builtin.value = result.builtin
    manual.value = result.manual
    expectedAddress.value = result.expectedAddress
    // 失效目录被主进程自动移除时说一声删了什么；其余情况提示清空，免得旧消息赖着不走
    notice.value = result.dropped.length
      ? `已移除失效目录：${result.dropped.map((entry) => entry.path).join('、')}`
      : ''
    loaded.value = true
  }

  async function reload() {
    try {
      // 两份一起取：目录检查与 daemon 死活是同一次刷新里的两半，分开取会闪
      const [result, daemonResult] = await Promise.all([
        window.electronAPI.getClientDirs(),
        window.electronAPI.getCodexDaemon()
      ])
      if (daemonResult?.success) daemon.value = daemonResult.daemon
      if (result.success) applyResult(result)
      else notice.value = result.error || '扫描客户端目录失败'
    } catch (e) {
      logger.error('扫描客户端目录失败', { error: e.message })
      notice.value = e.message || '扫描客户端目录失败'
    }
  }

  // 启动时 App 壳调一次：不用等设置页打开，顶栏的提示第一时间就能出现
  const init = () => reload()

  async function add() {
    notice.value = ''
    busy.value = true
    try {
      const picked = await window.electronAPI.pickClientDir()
      if (!picked.success) {
        notice.value = picked.error || '选择目录失败'
        return
      }
      if (!picked.path) return // 取消选择不算错，不提示
      const added = await window.electronAPI.addClientDir(picked.path)
      if (!added.success) {
        notice.value = added.error || '添加失败'
        return
      }
      await reload()
    } catch (e) {
      notice.value = e.message || '添加失败'
    } finally {
      busy.value = false
    }
  }

  async function remove(entry) {
    notice.value = ''
    const result = await window.electronAPI.removeClientDir(entry.id)
    if (!result.success) notice.value = result.error || '移除失败'
    await reload()
  }

  async function applyConfig(entry) {
    notice.value = ''
    busy.value = true
    try {
      const result = await window.electronAPI.applyClientDir(entry.path)
      if (!result.success) {
        notice.value = `${entry.path}：${result.error || '写入失败'}`
        return false
      }
      // 写完重新扫：检查结果变了，那一行的应用按钮与顶栏徽标都该跟着变
      await reload()
      return true
    } catch (e) {
      notice.value = e.message || '写入失败'
      return false
    } finally {
      busy.value = false
    }
  }

  return {
    // State
    builtin,
    manual,
    expectedAddress,
    daemon,
    notice,
    loaded,
    busy,
    // Computed
    problemCount,
    // Actions
    init,
    reload,
    add,
    remove,
    applyConfig
  }
})
