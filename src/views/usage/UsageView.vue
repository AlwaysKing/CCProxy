<script setup>
// 用量页：看这段时间打到上游的消耗。
// 聚合在主进程做（要读几十天日文件），本页只切区间维度
import { computed, onMounted, ref } from 'vue'
import { useUsageStore } from '@/stores/useUsageStore'
import { BaseTabs, EmptyState } from '@/components/base'
import { formatNumber, formatTokens } from '@/utils/format'
import UsageBar from './components/UsageBar.vue'

const usageStore = useUsageStore()

const rangeDays = ref(1)
const dimension = ref('day')

const RANGE_ITEMS = [
  { value: 1, label: '今天' },
  { value: 7, label: '近 7 天' },
  { value: 30, label: '近 30 天' }
]

const DIMENSION_ITEMS = [
  { value: 'day', label: '按天' },
  { value: 'provider', label: '按供应商' },
  { value: 'model', label: '按模型' }
]

// openai 是拆分前的协议名，历史用量记录里还有，标签表一并保留
const PROTOCOL_LABELS = {
  anthropic: 'Anthropic',
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  openai: 'OpenAI'
}

const DIMENSION_LABELS = { day: '日期', provider: '供应商', model: '模型' }

const EMPTY_TOTALS = { requests: 0, failed: 0, cancelled: 0, input: 0, output: 0, cacheRead: null }

const hasData = computed(() => (usageStore.totals?.requests ?? 0) > 0)

const summary = computed(() => {
  const totals = usageStore.totals || EMPTY_TOTALS
  return [
    { label: '请求数', value: formatNumber(totals.requests) },
    { label: '上游失败', value: formatNumber(totals.failed), tone: totals.failed > 0 ? 'error' : '' },
    { label: '已取消', value: formatNumber(totals.cancelled) },
    { label: '输入 tokens', value: formatTokens(totals.input), title: '含缓存命中与写入的全部输入' },
    { label: '缓存读', value: formatTokens(totals.cacheRead), title: '输入里命中缓存的部分（Anthropic 的 cache_read、OpenAI 的 cached_tokens）' },
    { label: '输出 tokens', value: formatTokens(totals.output) }
  ]
})

// 「输入」是含缓存的全部输入，缓存读写是它里面的明细：所以总账仍然是输入 + 输出，
// 缓存那一份已经在输入里了，再加一次就是重复计数
const protocolSegments = computed(() =>
  usageStore.byProtocol.map((item) => ({
    key: item.key,
    label: PROTOCOL_LABELS[item.key] || item.key,
    value: item.input + item.output
  }))
)

const modelSegments = computed(() =>
  usageStore.byModel
    .map((item) => ({ key: item.key, label: item.key, value: item.input + item.output }))
    .sort((a, b) => b.value - a.value)
)

// 三个维度一次都拿到了，切维度只是换一份数据，不再读盘
const rows = computed(() => {
  const source =
    dimension.value === 'provider' ? usageStore.byProvider : dimension.value === 'model' ? usageStore.byModel : usageStore.byDay
  return source.map((item) => ({ key: item.key, label: item.key, bucket: item }))
})

function onRangeChange() {
  usageStore.load({ days: rangeDays.value })
}

onMounted(() => {
  usageStore.load({ days: rangeDays.value })
})
</script>

<template>
  <div class="usage-page">
    <header class="page-bar">
      <BaseTabs v-model="rangeDays" :items="RANGE_ITEMS" size="sm" @change="onRangeChange" />
      <span class="bar-hint">
        {{ usageStore.isLoading ? '正在统计…' : '只统计真正转发到上游的请求' }}
      </span>
    </header>

    <p v-if="usageStore.error" class="page-notice">{{ usageStore.error }}</p>

    <div class="page-card summary">
      <div v-for="stat in summary" :key="stat.label" class="stat" :title="stat.title || ''">
        <span class="stat-label">{{ stat.label }}</span>
        <span class="stat-value" :class="stat.tone">{{ stat.value }}</span>
      </div>
    </div>

    <template v-if="hasData">
      <div class="page-card chart-card">
        <UsageBar title="两端协议消耗" :segments="protocolSegments" />
        <UsageBar title="各模型消耗" :segments="modelSegments" />
      </div>

      <div class="page-card">
        <div class="table-bar">
          <BaseTabs v-model="dimension" :items="DIMENSION_ITEMS" size="sm" />
        </div>

        <table class="data-table">
          <thead>
            <tr>
              <th>{{ DIMENSION_LABELS[dimension] }}</th>
              <th class="col-num">请求</th>
              <th class="col-num">失败</th>
              <th class="col-num">取消</th>
              <th class="col-num">输入</th>
              <th class="col-num" title="输入里命中缓存的部分">缓存读</th>
              <th class="col-num">输出</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in rows" :key="row.key">
              <td><code class="row-label">{{ row.label }}</code></td>
              <td class="col-num">{{ formatNumber(row.bucket.requests) }}</td>
              <td class="col-num" :class="{ 'cell-failed': row.bucket.failed > 0 }">
                {{ formatNumber(row.bucket.failed) }}
              </td>
              <td class="col-num cell-muted">{{ formatNumber(row.bucket.cancelled) }}</td>
              <!-- tokens 列跟上面的汇总卡片用同一套口径：超过万位就该收成 K/M，
                   不然一行十位数会把列撑歪。精确值挂在悬停上，不用为了看个全量数字去别处翻 -->
              <td class="col-num" :title="formatNumber(row.bucket.input)">{{ formatTokens(row.bucket.input) }}</td>
              <td class="col-num cell-muted" :title="formatNumber(row.bucket.cacheRead)">
                {{ formatTokens(row.bucket.cacheRead) }}
              </td>
              <td class="col-num" :title="formatNumber(row.bucket.output)">{{ formatTokens(row.bucket.output) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>

    <!-- 加载中不闪空态：首载或切区间时先只留汇总区，数据到了再出图表与表格 -->
    <div v-else-if="!usageStore.isLoading" class="page-card">
      <EmptyState
        title="这段时间还没有用量"
        description="请求转发到上游后会在这里记账；未转发（协议识别失败、映射未命中）的请求不计入"
      >
        <template #icon>
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <line x1="4" y1="20" x2="4" y2="10" />
            <line x1="12" y1="20" x2="12" y2="4" />
            <line x1="20" y1="20" x2="20" y2="14" />
          </svg>
        </template>
      </EmptyState>
    </div>
  </div>
</template>

<style scoped>
.usage-page {
  flex: 1;
  padding: var(--spacing-xl);
  display: flex;
  flex-direction: column;
  gap: var(--spacing-lg);
  overflow-y: auto;
}

.page-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-md);
  flex-wrap: wrap;
}

.bar-hint {
  color: var(--text-muted);
  font-size: var(--font-size-sm);
}

.page-notice {
  padding: var(--spacing-sm) var(--spacing-md);
  background: var(--color-error-bg);
  color: var(--color-error);
  border-radius: var(--radius-md);
  font-size: var(--font-size-sm);
}

/* 汇总：几个数并排，中间用竖线分开。分隔线比几个卡片省地方，
   也不会让「请求数 3」这种小数字被卡片边框框得像块砖 */
.summary {
  display: flex;
  align-items: stretch;
  flex-wrap: wrap;
  gap: var(--spacing-lg);
}

.stat {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-xs);
  min-width: 96px;
  flex: 1;
}

.stat + .stat {
  padding-left: var(--spacing-lg);
  border-left: 1px solid var(--border-default);
}

.stat-label {
  color: var(--text-muted);
  font-size: var(--font-size-xs);
}

.stat-value {
  color: var(--text-primary);
  font-size: var(--font-size-xl);
  font-family: var(--font-family-mono);
}

.stat-value.error {
  color: var(--color-error);
}

/* 两条并排：协议只有两三种、模型可能十几个，各自需要的宽度不一样，
   挤在一列里图例会绕得很难读 */
.chart-card {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: var(--spacing-xl);
}

.table-bar {
  margin-bottom: var(--spacing-md);
}

.row-label {
  font-family: var(--font-family-mono);
  color: var(--text-primary);
}

/* 失败数只在真的失败时显红：全是 0 的一列红字会让整张表看着都在报警 */
.cell-failed {
  color: var(--color-error);
}

.cell-muted {
  color: var(--text-muted);
}

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
