<script setup>
// 设置页：端口、客户端 token、开机自启、客户端目录、数据存储。
// 端口保存后由主进程判断要不要按新端口重启（applyConfig 的副作用）——这里不发重启指令，
// 托盘或别处改配置走的也是同一条路，两边行为必须一致
import { computed, onMounted, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { useProxyStore } from '@/stores/useProxyStore'
import { useClientDirsStore } from '@/stores/useClientDirsStore'
import { BaseButton, BaseInput, ConfirmDialog, IconButton, Toggle } from '@/components/base'
import { formatBytes } from '@/utils/format'

const configStore = useConfigStore()
const proxyStore = useProxyStore()
const clientDirsStore = useClientDirsStore()

const portDraft = ref('')
const notice = ref('')
const stats = ref(null)
const clearing = ref(false)
const confirmingClear = ref(null) // 'logs' | 'usage' | 'token' | null

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
  // 目录检查是拿当前端口判的，端口一改结论就过期了，重扫一遍
  if (ok) await clientDirsStore.reload()
}

const launchAtLogin = computed(() => configStore.config?.settings.launchAtLogin === true)

async function onToggleLaunchAtLogin(value) {
  notice.value = ''
  const ok = await configStore.updateSetting('launchAtLogin', value)
  if (!ok) notice.value = configStore.error || '保存失败'
}

// 请求日志 / 用量记录的记录开关（缺省即开启）。关掉后主进程不再落对应数据，历史保留
const logsEnabled = computed(() => configStore.config?.settings.logsEnabled !== false)
const usageEnabled = computed(() => configStore.config?.settings.usageEnabled !== false)

async function onToggleStorage(settingKey, value) {
  notice.value = ''
  const ok = await configStore.updateSetting(settingKey, value)
  if (!ok) notice.value = configStore.error || '保存失败'
}

// 客户端 token 由主进程生成并落盘，这里只展示与复制；重新生成也走主进程
// （applyConfig 会广播 config-changed，configStore 自动跟上）
const clientToken = computed(() => configStore.config?.settings.clientToken ?? '')
const showToken = ref(false)

async function onCopyToken() {
  if (!clientToken.value) return
  try {
    await navigator.clipboard.writeText(clientToken.value)
  } catch (e) {
    notice.value = '复制失败'
  }
}

async function loadStats() {
  const result = await window.electronAPI.getStorageStats()
  if (result.success) stats.value = result
}

// 客户端配置目录：列表与检查结果都在 clientDirsStore（顶栏徽标也吃这一份）。
// App 壳启动时 init 过一次，这里只在挂载时刷新一遍——挂载和启动之间用户可能刚改过目录
const expectedAddress = computed(() => clientDirsStore.expectedAddress)
const dirBusy = computed(() => clientDirsStore.busy)
const dirNotice = computed(() => clientDirsStore.notice)

const DIR_GROUP_META = [
  { kind: 'claude', label: 'Claude' },
  { kind: 'codex', label: 'Codex' }
]

const groupedDirs = computed(() => {
  const groups = { claude: [], codex: [] }
  for (const entry of clientDirsStore.builtin) groups[entry.kind]?.push({ ...entry, source: '内置' })
  for (const entry of clientDirsStore.manual) groups[entry.kind]?.push({ ...entry, source: '手动' })
  return groups
})

const onAddDir = () => clientDirsStore.add()
const onRemoveDir = (entry) => clientDirsStore.remove(entry)
const onApplyDir = (entry) => clientDirsStore.applyConfig(entry)

// codex 的 app-server daemon 只在启动时把 config.toml 与模型清单读进内存：
// CCProxy 写的是磁盘，daemon 手里那份还是旧的，所以得提醒用户重启它才会生效
const codexDaemon = computed(() => clientDirsStore.daemon)

async function onCopyRestartCommand() {
  const command = codexDaemon.value?.restartCommand || 'codex app-server daemon restart'
  try {
    await navigator.clipboard.writeText(command)
  } catch (e) {
    notice.value = '复制失败'
  }
}

// 两种数据的清理语义不同（见 request-log 的注释），所以分两个入口而不是一个「清空数据」
const STORAGE_ROWS = computed(() => [
  {
    key: 'logs',
    name: '请求日志',
    settingKey: 'logsEnabled',
    enabled: logsEnabled.value,
    desc: '每个请求的完整记录（含识别打分、错误详情），按天分文件',
    files: stats.value?.logs?.files ?? 0,
    bytes: stats.value?.logs?.bytes ?? 0
  },
  {
    key: 'usage',
    name: '用量记录',
    settingKey: 'usageEnabled',
    enabled: usageEnabled.value,
    desc: '转发到上游的请求的 tokens 记账，按天分文件；清掉后统计从零开始',
    files: stats.value?.usage?.files ?? 0,
    bytes: stats.value?.usage?.bytes ?? 0
  }
])

const CLEAR_META = {
  logs: { title: '清空请求日志', description: '会删掉落盘的全部历史日志，不影响用量统计。内存里的最近 500 条会在下次打开日志页时重新加载。确定清空？', confirmText: '清空' },
  usage: { title: '清空用量记录', description: '会删掉全部用量历史，之后统计从零开始，请求日志不受影响。确定清空？', confirmText: '清空' },
  token: { title: '重新生成客户端 token', description: '会换一个新的 token；原本一致的客户端配置目录会自动写入新值，原本就有问题的保持原样，仍需手动处理。确定重新生成？', confirmText: '重新生成' }
}

async function onClear() {
  const key = confirmingClear.value
  clearing.value = true
  try {
    if (key === 'token') {
      const result = await window.electronAPI.regenerateClientToken()
      if (!result.success) notice.value = result.error || '重新生成失败'
      else {
        // 主进程已落盘并广播，直接回填拿最新值，不赌广播与响应的到达顺序
        configStore.applyState({ config: result.config })
        showToken.value = true
        notice.value = result.errors?.length
          ? `已重新生成；${result.reapplied?.length ?? 0} 个目录已自动写入，${result.errors.length} 个写入失败：${result.errors[0].message}`
          : `已重新生成；原本一致的 ${result.reapplied?.length ?? 0} 个目录已自动写入新 token`
        // token 一换检查结论就过期了，重扫一遍（与改端口同一条路）
        await clientDirsStore.reload()
      }
      confirmingClear.value = null
      return
    }
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

onMounted(() => {
  loadStats()
  clientDirsStore.reload()
})
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
          <BaseInput v-model="portDraft" type="number" size="sm" class="port-input" :error="!!portError && portChanged" />
          <BaseButton
            variant="primary"
            size="sm"
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
          <span class="setting-name">客户端 token</span>
          <span class="setting-desc">客户端连 CCProxy 的凭证，配置客户端时填它；重新生成后，原本一致的目录会自动写入新值，原本有问题的保持原样</span>
        </div>
        <div class="setting-control">
          <span class="token-value" :title="showToken ? clientToken : ''">{{ showToken ? clientToken : '••••••••••••••••' }}</span>
          <IconButton size="sm" :title="showToken ? '隐藏' : '显示'" @click="showToken = !showToken">
            <svg v-if="showToken" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94" />
              <path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19" />
              <line x1="1" y1="1" x2="23" y2="23" />
            </svg>
            <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
          </IconButton>
          <IconButton size="sm" title="复制" @click="onCopyToken">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
            </svg>
          </IconButton>
          <BaseButton variant="secondary" size="sm" @click="confirmingClear = 'token'">重新生成</BaseButton>
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
      <div class="dirs-head">
        <div class="setting-info">
          <span class="setting-name">客户端配置目录</span>
          <span class="setting-desc">扫描到的 Claude / Codex 配置目录；内置位置自动发现，手动添加的目录失效后会被自动移除</span>
          <span v-if="expectedAddress" class="setting-desc">检查口径：指向 <code>{{ expectedAddress }}</code> 且 token 与 CCProxy 一致；Codex 还会对照模型清单（models.json）与 OpenAI 协议映射。不一致的行可点「应用」写入</span>
        </div>
        <BaseButton size="sm" :loading="dirBusy" @click="onAddDir">+ 添加目录</BaseButton>
      </div>

      <div v-for="group in DIR_GROUP_META" :key="group.kind" class="dirs-group">
        <span class="dirs-group-title">{{ group.label }}</span>
        <div v-if="groupedDirs[group.kind].length" class="dirs-list">
          <div v-for="entry in groupedDirs[group.kind]" :key="entry.path" class="dirs-item">
            <div class="dirs-row">
              <span class="dirs-path" :title="entry.path">{{ entry.path }}</span>
              <span class="dirs-tag">{{ entry.source }}</span>
              <IconButton v-if="entry.source === '手动'" class="dirs-remove" size="sm" danger title="移除该目录" @click="onRemoveDir(entry)">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <polyline points="3 6 5 6 21 6" />
                  <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
                  <path d="M10 11v6M14 11v6" />
                  <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                </svg>
              </IconButton>
              <!-- 只有不一致才出现，直接点了把 CCProxy 的地址与 token 写进去。
                   差在哪放在悬停提示里：列表上不铺检查结果，但点之前得能看明白要改什么 -->
              <BaseButton
                v-if="entry.status !== 'ok'"
                class="dirs-apply"
                size="sm"
                :disabled="dirBusy"
                :title="entry.problems?.join('\n') || ''"
                @click="onApplyDir(entry)"
              >应用</BaseButton>
            </div>
          </div>
        </div>
        <span v-else class="dirs-empty">未发现</span>
        <!-- codex 侧还有一层缓存：长期在跑的 app-server daemon。写到磁盘的东西它不重读，
             所以这一行要留在目录列表里——用户「改了没生效」时第一眼能看见怎么办 -->
        <p
          v-if="group.kind === 'codex' && codexDaemon"
          class="dirs-daemon"
          :class="{ 'dirs-daemon-live': codexDaemon.running }"
        >
          <span class="dirs-daemon-text">
            <template v-if="codexDaemon.running">Codex 的 app-server daemon 正在运行（PID {{ codexDaemon.pid }}），它只在启动时读一次 config.toml 与模型清单：改完地址 / token / 模型后要执行</template>
            <template v-else>如果 Codex 侧在用 app-server daemon，改完地址 / token / 模型后要执行</template>
            <code>{{ codexDaemon.restartCommand }}</code>
            才会生效（正在跑的会话会被打断）
          </span>
          <IconButton size="sm" :title="`复制 ${codexDaemon.restartCommand}`" @click="onCopyRestartCommand">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
            </svg>
          </IconButton>
        </p>
      </div>

      <p v-if="dirNotice" class="dirs-notice">{{ dirNotice }}</p>
    </div>

    <div class="page-card">
      <div v-for="row in STORAGE_ROWS" :key="row.key" class="setting-row">
        <div class="setting-info">
          <span class="setting-name">{{ row.name }}</span>
          <span class="setting-desc">{{ row.desc }}</span>
        </div>
        <div class="setting-control">
          <Toggle
            :model-value="row.enabled"
            :disabled="configStore.saving"
            @change="onToggleStorage(row.settingKey, $event)"
          />
          <span class="storage-size">{{ row.files ? `${row.files} 个文件 · ${formatBytes(row.bytes)}` : '暂无数据' }}</span>
          <BaseButton
            variant="secondary"
            size="sm"
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
      :confirm-text="CLEAR_META[confirmingClear]?.confirmText || '确认'"
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

.setting-desc code {
  font-family: var(--font-family-mono);
  color: var(--text-secondary);
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

/* 客户端目录卡：标题行左边说明右边添加按钮，下面 claude/codex 各一组 */
.dirs-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-lg);
  padding: var(--spacing-md) 0;
}

.dirs-group {
  padding: var(--spacing-sm) 0;
}

.dirs-group + .dirs-group,
.dirs-group + .dirs-notice {
  border-top: 1px solid var(--border-default);
}

.dirs-group-title {
  display: block;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  font-weight: var(--font-weight-medium);
  margin-bottom: var(--spacing-xs);
}

.dirs-list {
  display: flex;
  flex-direction: column;
}

.dirs-row {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  padding: 2px 0;
}

/* 两个按钮都靠右：先出现的那一个吃掉剩余空间，于是「应用」永远贴行尾。
   只有其中一个存在时也各自靠右，不会因为按钮多少而左右跳 */
.dirs-remove,
.dirs-apply {
  margin-left: auto;
}

/* 目录行是 xs 字号的紧凑行，「应用」用 btn-sm 会把行高撑出一截，
   压回与行内标签（内置/手动）同一档 */
.dirs-apply {
  padding: 1px 8px;
  font-size: var(--font-size-xs);
}

.dirs-path {
  flex: 1 1 auto;
  min-width: 0;
  color: var(--text-primary);
  font-family: var(--font-family-mono);
  font-size: var(--font-size-xs);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dirs-tag {
  flex: 0 0 auto;
  padding: 1px 6px;
  background: var(--bg-tertiary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}

.dirs-empty {
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}

/* codex daemon 提醒：没检测到 daemon 时是灰色说明，检测到了才提色提醒 */
.dirs-daemon {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  margin: var(--spacing-xs) 0 0;
  padding-top: var(--spacing-xs);
  border-top: 1px solid var(--border-default);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}

.dirs-daemon-live {
  color: var(--color-warning);
}

.dirs-daemon-text {
  flex: 1 1 auto;
  line-height: 1.6;
}

/* 命令要能一眼抄走：等宽 + 底衬，和正文拉开距离 */
.dirs-daemon code {
  margin: 0 4px;
  padding: 1px 6px;
  background: var(--bg-tertiary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  font-family: var(--font-family-mono);
}

.dirs-notice {
  margin: var(--spacing-sm) 0 0;
  padding-top: var(--spacing-sm);
  color: var(--color-warning);
  font-size: var(--font-size-sm);
}

.port-input {
  width: 120px;
}

/* token 整串亮出来，不截断：要看完整值就得复制级别的完整，截一半没有意义 */
.token-value {
  white-space: nowrap;
  color: var(--text-secondary);
  font-family: var(--font-family-mono);
  font-size: var(--font-size-sm);
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
