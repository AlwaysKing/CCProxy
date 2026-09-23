<script setup>
// 设置页：端口、开机自启、数据存储。
// 端口保存后由主进程判断要不要按新端口重启（applyConfig 的副作用）——这里不发重启指令，
// 托盘或别处改配置走的也是同一条路，两边行为必须一致
import { computed, onMounted, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { useProxyStore } from '@/stores/useProxyStore'
import { BaseButton, BaseInput, ConfirmDialog, Toggle } from '@/components/base'
import { formatBytes } from '@/utils/format'

const configStore = useConfigStore()
const proxyStore = useProxyStore()

const portDraft = ref('')
const notice = ref('')
const stats = ref(null)
const clearing = ref(false)
const confirmingClear = ref(null) // 'logs' | 'usage' | null

const port = computed(() => configStore.config?.settings.port ?? null)
// 配置里改了端口（哪怕不是在本页改的）要跟着刷草稿，否则这里显示的还是旧值
watch(port, (value) => { portDraft.value = value === null ? '' : String(value) }, { immediate: true })

const portChanged = computed(() => portDraft.value !== String(port.value ?? ''))
const portValid = computed(() => {
  const value = Number(portDraft.value)
  return Number.isInteger(value) && value >= 1 && value <= 65535
})
const portError = computed(() => {
  if (!portDraft.value) return portChanged.value ? '端口不能为空' : ''
  return portValid.value ? '' : '端口要在 1-65535 之间'
})

async function onSavePort() {
  if (!portValid.value || !portChanged.value) return
  notice.value = ''
  const ok = await configStore.updateSetting('port', Number(portDraft.value))
  if (!ok) notice.value = configStore.error || '保存失败'
  else if (proxyStore.status === 'running') notice.value = '已保存，代理正在按新端口重启'
}

const launchAtLogin = computed(() => configStore.config?.settings.launchAtLogin === true)

async function onToggleLaunchAtLogin(value) {
  notice.value = ''
  const ok = await configStore.updateSetting('launchAtLogin', value)
  if (!ok) notice.value = configStore.error || '保存失败'
}

async function loadStats() {
  const result = await window.electronAPI.getStorageStats()
  if (result.success) stats.value = result
}

// 两种数据的清理语义不同（见 request-log 的注释），所以分两个入口而不是一个「清空数据」
const STORAGE_ROWS = computed(() => [
  {
    key: 'logs',
    name: '请求日志',
    desc: '每个请求的完整记录（含识别打分、错误详情），按天分文件',
    files: stats.value?.logs?.files ?? 0,
    bytes: stats.value?.logs?.bytes ?? 0
  },
  {
    key: 'usage',
    name: '用量记录',
    desc: '转发到上游的请求的 tokens 记账，按天分文件；清掉后统计与费用从零开始',
    files: stats.value?.usage?.files ?? 0,
    bytes: stats.value?.usage?.bytes ?? 0
  }
])

const CLEAR_META = {
  logs: { title: '清空请求日志', description: '会删掉落盘的全部历史日志，不影响用量统计。内存里的最近 500 条会在下次打开日志页时重新加载。确定清空？' },
  usage: { title: '清空用量记录', description: '会删掉全部用量历史，之后统计与费用从零开始，请求日志不受影响。确定清空？' }
}

async function onClear() {
  const key = confirmingClear.value
  clearing.value = true
  try {
    const result = key === 'usage'
      ? await window.electronAPI.clearUsage()
      : await window.electronAPI.clearLogs()
    if (!result.success) notice.value = result.error || '清理失败'
    confirmingClear.value = null
    await loadStats()
  } finally {
    clearing.value = false
  }
}

onMounted(loadStats)
</script>

<template>
  <div class="settings-page">
    <p v-if="notice" class="page-notice">{{ notice }}</p>

    <div class="page-card">
      <div class="setting-row">
        <div class="setting-info">
          <span class="setting-name">代理端口</span>
          <span class="setting-desc">客户端连这个端口。保存后若代理正在运行，会自动按新端口重启</span>
          <p v-if="portError && portChanged" class="setting-error">{{ portError }}</p>
        </div>
        <div class="setting-control">
          <BaseInput v-model="portDraft" type="number" class="port-input" :error="!!portError && portChanged" />
          <BaseButton
            variant="primary"
            size="md"
            :disabled="!portChanged || !portValid"
            :loading="configStore.saving"
            @click="onSavePort"
          >
            保存
          </BaseButton>
        </div>
      </div>

      <div class="setting-row">
        <div class="setting-info">
          <span class="setting-name">开机自启</span>
          <span class="setting-desc">登录 macOS 后自动启动 CCProxy（同步到系统设置的用户与群组登录项）</span>
        </div>
        <div class="setting-control">
          <Toggle :model-value="launchAtLogin" :disabled="configStore.saving" @change="onToggleLaunchAtLogin" />
        </div>
      </div>
    </div>

    <div class="page-card">
      <div v-for="row in STORAGE_ROWS" :key="row.key" class="setting-row">
        <div class="setting-info">
          <span class="setting-name">{{ row.name }}</span>
          <span class="setting-desc">{{ row.desc }}</span>
        </div>
        <div class="setting-control">
          <span class="storage-size">{{ row.files ? `${row.files} 个文件 · ${formatBytes(row.bytes)}` : '暂无数据' }}</span>
          <BaseButton
            variant="secondary"
            size="md"
            :disabled="!row.files"
            @click="confirmingClear = row.key"
          >
            清空
          </BaseButton>
        </div>
      </div>

      <p v-if="stats?.dir" class="storage-dir">
        数据目录 <code>{{ stats.dir }}</code>（配置 app.json · 日志 logs/ · 用量 usage/）
      </p>
    </div>

    <ConfirmDialog
      :model-value="!!confirmingClear"
      :title="CLEAR_META[confirmingClear]?.title || '确认操作'"
      :description="CLEAR_META[confirmingClear]?.description || ''"
      confirm-text="清空"
      :loading="clearing"
      @update:model-value="confirmingClear = null"
      @confirm="onClear"
    />
  </div>
</template>

<style scoped>
.settings-page {
  flex: 1;
  padding: var(--spacing-xl);
  display: flex;
  flex-direction: column;
  gap: var(--spacing-lg);
  overflow-y: auto;
}

.page-notice {
  padding: var(--spacing-sm) var(--spacing-md);
  background: var(--color-warning-bg);
  color: var(--color-warning);
  border-radius: var(--radius-md);
  font-size: var(--font-size-sm);
}

/* 一行一个设置：左边名字与说明，右边控件。行间画线分组，比每行一张卡片轻 */
.setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-xl);
  padding: var(--spacing-md) 0;
}

.setting-row + .setting-row {
  border-top: 1px solid var(--border-default);
}

.setting-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.setting-name {
  color: var(--text-primary);
  font-size: var(--font-size-base);
  font-weight: var(--font-weight-medium);
}

.setting-desc {
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}

.setting-error {
  margin-top: var(--spacing-xs);
  color: var(--color-error);
  font-size: var(--font-size-sm);
}

.setting-control {
  display: flex;
  align-items: center;
  gap: var(--spacing-md);
  flex-shrink: 0;
}

.port-input {
  width: 120px;
}

.storage-size {
  color: var(--text-muted);
  font-size: var(--font-size-sm);
  font-family: var(--font-family-mono);
}

.storage-dir {
  margin-top: var(--spacing-md);
  padding-top: var(--spacing-md);
  border-top: 1px solid var(--border-default);
  color: var(--text-muted);
  font-size: var(--font-size-sm);
}

.storage-dir code {
  font-family: var(--font-family-mono);
  color: var(--text-secondary);
}
</style>
