<script setup>
// 请求日志页：看代理最近收到了什么、走到哪一步、上游回得怎么样。
// 过滤在本地做——最多 500 条，比来回跑 IPC 快也比它简单。
// 进页面拉一次全量，之后靠推送增量；离开页面就退订，不留后台监听
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useLogsStore } from '@/stores/useLogsStore'
import { Badge, BaseButton, BaseDialog, BaseTabs, ConfirmDialog, EmptyState } from '@/components/base'
import { formatClock, formatDateTime, formatDuration, formatNumber, formatTokens } from '@/utils/format'

const logsStore = useLogsStore()

const protocolFilter = ref('all')
const statusFilter = ref('all')
const detail = ref(null)
const confirmingClear = ref(false)
let unsubscribe = null

const PROTOCOL_ITEMS = [
  { value: 'all', label: '全部协议' },
  { value: 'anthropic', label: 'Anthropic' },
  { value: 'openai_chat', label: 'OpenAI Chat' },
  { value: 'openai_responses', label: 'OpenAI Responses' }
]

const STATUS_ITEMS = [
  { value: 'all', label: '全部状态' },
  { value: 'ok', label: '成功' },
  { value: 'failed', label: '失败' },
  { value: 'cancelled', label: '已取消' }
]

// 老日志里 openai 是一条协议，拆分前的记录还挂着这个值，所以标签表要留着它
const PROTOCOL_LABELS = {
  anthropic: 'Anthropic',
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  openai: 'OpenAI'
}

// 规则用词必须和映射模块一致（精确/通配/兜底）：两处说法不同，会让人以为是两回事
const RULE_META = {
  exact: { label: '精确', variant: 'default' },
  wildcard: { label: '通配', variant: 'warning' },
  fallback: { label: '兜底', variant: 'primary' }
}

// 走到了哪一步。三个未转发的分支（收体失败 / 识别失败 / 查表失败）没有供应商与模型，那几列会是空的
const OUTCOME_LABELS = {
  models: '模型列表（代理接管）',
  read_failed: '请求体读取失败',
  detect_failed: '协议识别失败',
  rejected: '未转发（映射未命中或协议停用）',
  forwarded: '已转发上游'
}

// 没打到上游的失败（收体失败 / 识别失败 / 未命中）状态码同样是 4xx/5xx，
// 跟上游 4xx 混一起看不出是谁的问题，标个「本地」区分
function isLocalOutcome(entry) {
  return entry.outcome === 'read_failed' || entry.outcome === 'detect_failed' || entry.outcome === 'rejected'
}

// 状态分类与用量页口径一致：客户端取消不算失败——用户自己按了 Esc 不该算上游出错
function statusKind(entry) {
  if (entry.cancelled) return { key: 'cancelled', label: '已取消', variant: 'default' }
  const ok = entry.statusCode >= 200 && entry.statusCode < 300
  return { key: ok ? 'ok' : 'failed', label: String(entry.statusCode || '—'), variant: ok ? 'success' : 'error' }
}

function protocolLabel(entry) {
  return PROTOCOL_LABELS[entry.protocol] || '未识别'
}

const filtered = computed(() =>
  logsStore.entries.filter((entry) => {
    if (protocolFilter.value !== 'all' && entry.protocol !== protocolFilter.value) return false
    if (statusFilter.value !== 'all' && statusKind(entry).key !== statusFilter.value) return false
    return true
  })
)

// 输入是含缓存的全部输入，命中缓存的那部分跟在后面当明细；没上报的就只报输入与输出
function tokenSummary(tokens) {
  const head = `输入 ${formatNumber(tokens.input)} / 输出 ${formatNumber(tokens.output)}`
  if (tokens.cacheRead === null || tokens.cacheRead === undefined) return head
  return `${head}（缓存读 ${formatNumber(tokens.cacheRead)}）`
}

// 详情按行铺开：字段名与日志 entry 一一对应，排查时对着这里就能说出「当时到底怎么了」
const detailRows = computed(() => {
  const entry = detail.value
  if (!entry) return []
  const rule = RULE_META[entry.matchKind]
  return [
    { label: '时间', value: formatDateTime(entry.timestamp) },
    { label: '请求', value: `${entry.method || '—'} ${entry.path || ''}`.trim(), mono: true },
    { label: '协议', value: protocolLabel(entry) },
    { label: '结果', value: OUTCOME_LABELS[entry.outcome] || entry.outcome },
    { label: '映射模型', value: entry.clientModel || '—', mono: true },
    { label: '命中规则', value: entry.matchedRule ? `${rule ? rule.label : '命中'} ${entry.matchedRule}` : '—', mono: !!entry.matchedRule },
    { label: '供应商', value: entry.providerName ? `${entry.providerName}（${entry.providerId}）` : '—' },
    { label: '原始模型', value: entry.realModel || '—', mono: true },
    { label: '状态码', value: String(entry.statusCode || '—') },
    { label: '耗时', value: formatDuration(entry.durationMs) },
    {
      label: 'tokens',
      value: entry.tokens ? tokenSummary(entry.tokens) : '上游未上报'
    },
    { label: '错误', value: entry.error || '—' }
  ]
})

async function onClear() {
  const ok = await logsStore.clear()
  if (ok) confirmingClear.value = false
}

onMounted(() => {
  logsStore.init()
  unsubscribe = window.electronEvents.onLogAppended((entry) => logsStore.applyEntry(entry))
})

onUnmounted(() => {
  if (unsubscribe) unsubscribe()
})
</script>

<template>
  <div class="logs-page">
    <header class="page-bar">
      <div class="bar-filters">
        <BaseTabs v-model="protocolFilter" :items="PROTOCOL_ITEMS" size="sm" />
        <BaseTabs v-model="statusFilter" :items="STATUS_ITEMS" size="sm" />
      </div>
      <div class="bar-actions">
        <span class="bar-count">
          {{ logsStore.entries.length ? `显示 ${filtered.length} / 共 ${logsStore.entries.length} 条` : '' }}
        </span>
        <BaseButton
          variant="secondary"
          size="sm"
          :disabled="!logsStore.entries.length"
          @click="confirmingClear = true"
        >
          清空日志
        </BaseButton>
      </div>
    </header>

    <div class="page-card">
      <p v-if="logsStore.error" class="page-notice">{{ logsStore.error }}</p>

      <table v-if="filtered.length" class="data-table">
        <thead>
          <tr>
            <th>时间</th>
            <th>协议</th>
            <th>映射模型</th>
            <th>供应商</th>
            <th>原始模型</th>
            <th>状态</th>
            <th class="col-num">耗时</th>
            <th class="col-num">输入 / 输出</th>
            <th class="col-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="entry in filtered" :key="entry.id || entry.timestamp">
            <td class="cell-time">{{ formatClock(entry.timestamp) }}</td>
            <td>
              <span class="cell-protocol" :class="{ muted: !entry.protocol }">{{ protocolLabel(entry) }}</span>
            </td>
            <!-- 映射模型与规则徽章同一格：规则是「这个模型名是靠哪种方式命中的」，脱离模型名没意义 -->
            <td>
              <div class="cell-model">
                <code v-if="entry.clientModel">{{ entry.clientModel }}</code>
                <span v-else class="cell-empty">—</span>
                <Badge v-if="RULE_META[entry.matchKind]" :variant="RULE_META[entry.matchKind].variant" size="sm">
                  {{ RULE_META[entry.matchKind].label }}
                </Badge>
              </div>
            </td>
            <td>
              <span v-if="entry.providerName">{{ entry.providerName }}</span>
              <span v-else class="cell-empty">—</span>
            </td>
            <td>
              <code v-if="entry.realModel" class="cell-real">{{ entry.realModel }}</code>
              <span v-else class="cell-empty">—</span>
            </td>
            <td>
              <Badge :variant="statusKind(entry).variant" size="sm">{{ statusKind(entry).label }}</Badge>
              <span v-if="isLocalOutcome(entry)" class="cell-tag">本地</span>
            </td>
            <td class="col-num">{{ formatDuration(entry.durationMs) }}</td>
            <td class="col-num cell-tokens">
              <template v-if="entry.tokens">
                {{ formatTokens(entry.tokens.input) }} / {{ formatTokens(entry.tokens.output) }}
              </template>
              <span v-else class="cell-empty">—</span>
            </td>
            <td class="col-actions">
              <BaseButton variant="ghost" size="sm" @click="detail = entry">详情</BaseButton>
            </td>
          </tr>
        </tbody>
      </table>

      <EmptyState
        v-else-if="logsStore.entries.length"
        title="没有符合过滤条件的请求"
        description="换一个协议或状态看看"
      >
        <template #icon>
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
            <circle cx="11" cy="11" r="7" />
            <line x1="21" y1="21" x2="16.7" y2="16.7" />
          </svg>
        </template>
      </EmptyState>

      <EmptyState
        v-else
        title="还没有请求记录"
        description="代理收到请求后这里会实时出现，最多保留最近 500 条；历史全量在同目录的 logs/ 里"
      >
        <template #icon>
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
            <line x1="8" y1="13" x2="16" y2="13" />
            <line x1="8" y1="17" x2="13" y2="17" />
          </svg>
        </template>
      </EmptyState>
    </div>

    <BaseDialog
      :model-value="!!detail"
      title="请求详情"
      width="560px"
      @update:model-value="detail = null"
    >
      <div v-if="detail" class="detail">
        <div v-for="row in detailRows" :key="row.label" class="detail-row">
          <span class="detail-label">{{ row.label }}</span>
          <span class="detail-value" :class="{ mono: row.mono }">{{ row.value }}</span>
        </div>
      </div>
    </BaseDialog>

    <ConfirmDialog
      :model-value="confirmingClear"
      title="清空请求日志"
      description="会清掉内存里的 500 条和落盘的历史日志，用量统计不受影响。确定清空？"
      confirm-text="清空"
      :loading="logsStore.isLoading"
      @update:model-value="confirmingClear = false"
      @confirm="onClear"
    />
  </div>
</template>

<style scoped>
.logs-page {
  flex: 1;
  padding: var(--spacing-xl);
  display: flex;
  flex-direction: column;
  gap: var(--spacing-lg);
  overflow-y: auto;
}

/* 筛选条与卡片同宽：左边一组筛选、右边计数与清空，和卡片首列对在同一条竖线上 */
.page-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-md);
  flex-wrap: wrap;
}

.bar-filters {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

.bar-actions {
  display: flex;
  align-items: center;
  gap: var(--spacing-md);
}

.bar-count {
  color: var(--text-muted);
  font-size: var(--font-size-sm);
}

.page-notice {
  margin-bottom: var(--spacing-md);
  padding: var(--spacing-sm) var(--spacing-md);
  background: var(--color-error-bg);
  color: var(--color-error);
  border-radius: var(--radius-md);
  font-size: var(--font-size-sm);
}

.cell-time {
  color: var(--text-secondary);
  font-family: var(--font-family-mono);
  white-space: nowrap;
}

.cell-protocol {
  color: var(--text-primary);
}

.cell-protocol.muted {
  color: var(--text-muted);
}

/* 和映射模块的写法一致：模型名都是等宽的，规则徽章跟在名字后面 */
.cell-model {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

.cell-model code,
.cell-real {
  font-family: var(--font-family-mono);
  color: var(--text-primary);
}

.cell-empty {
  color: var(--text-muted);
}

/* 未转发的请求不该和上游 4xx 混为一谈，给个小标记，扫一眼就知道是谁的锅 */
.cell-tag {
  margin-left: var(--spacing-sm);
  padding: 0 4px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  font-size: var(--font-size-xs);
}

.cell-tokens {
  color: var(--text-secondary);
  white-space: nowrap;
}

.col-actions {
  width: 64px;
  text-align: right;
  white-space: nowrap;
}

/* ===== 详情弹窗 ===== */

.detail-row {
  display: flex;
  gap: var(--spacing-md);
  padding: var(--spacing-xs) 0;
  font-size: var(--font-size-sm);
}

.detail-label {
  flex: 0 0 72px;
  color: var(--text-muted);
}

.detail-value {
  flex: 1;
  color: var(--text-primary);
  word-break: break-all;
}

.detail-value.mono {
  font-family: var(--font-family-mono);
}

/* 空态在卡片里收一档，与模块卡片里的空态同高 */
.page-card :deep(.empty-state) {
  padding: var(--spacing-lg) var(--spacing-md);
}

.page-card :deep(.empty-icon) {
  margin-bottom: var(--spacing-sm);
  color: var(--text-secondary);
  opacity: 0.8;
}

.page-card :deep(.empty-title) {
  color: var(--text-primary);
}

.page-card :deep(.empty-description) {
  margin-bottom: 0;
  color: var(--text-secondary);
}
</style>
