<script setup>
// 用量页：看这段时间打到上游的消耗与花费。
// 聚合与计价都在主进程做（要读几十天日文件、要用最新的单价），本页只切区间维度、配单价。
// 单价从这里写进 settings.pricing，写完重新聚合一次——历史费用立刻按新单价重算
import { computed, onMounted, ref } from 'vue'
import { useUsageStore } from '@/stores/useUsageStore'
import { useConfigStore } from '@/stores/useConfigStore'
import { BaseButton, BaseDialog, BaseInput, BaseTabs, EmptyState, FormField } from '@/components/base'
import { formatMoney, formatNumber, formatTokens } from '@/utils/format'
import UsageBar from './components/UsageBar.vue'

const usageStore = useUsageStore()
const configStore = useConfigStore()

const rangeDays = ref(1)
const dimension = ref('day')
const pricingModel = ref(null)
const pricingForm = ref({ input: '', output: '' })
const pricingError = ref('')

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

const PROTOCOL_LABELS = { anthropic: 'Anthropic', openai: 'OpenAI' }

const DIMENSION_LABELS = { day: '日期', provider: '供应商', model: '模型' }

const EMPTY_TOTALS = { requests: 0, failed: 0, cancelled: 0, input: 0, output: 0, cost: 0, priced: 0 }

const hasData = computed(() => (usageStore.totals?.requests ?? 0) > 0)

// 有模型没配单价时费用只是部分的：界面上加星号并在表下说明，别让人把不完整的金额当真
function costText(bucket) {
  if (!bucket || !bucket.priced) return '—'
  return formatMoney(bucket.cost)
}

function isPartialCost(bucket) {
  return !!bucket && bucket.priced > 0 && bucket.priced < bucket.requests
}

function pricingOf(model) {
  return configStore.config?.settings?.pricing?.[model] || null
}

const summary = computed(() => {
  const totals = usageStore.totals || EMPTY_TOTALS
  const partial = isPartialCost(totals)
  return [
    { label: '请求数', value: formatNumber(totals.requests) },
    { label: '上游失败', value: formatNumber(totals.failed), tone: totals.failed > 0 ? 'error' : '' },
    { label: '已取消', value: formatNumber(totals.cancelled) },
    { label: '输入 tokens', value: formatTokens(totals.input) },
    { label: '输出 tokens', value: formatTokens(totals.output) },
    {
      label: '费用',
      value: `${costText(totals)}${partial ? '*' : ''}`,
      tone: totals.priced > 0 ? 'accent' : '',
      title: totals.priced > 0 ? '按 settings.pricing 里的单价算' : '还没给任何模型配单价'
    }
  ]
})

// 消耗按「输入 + 输出」算总账：图表比的是谁吃得多，只算输出会把输入大的模型看小
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

const hasPartialCost = computed(() => rows.value.some((row) => isPartialCost(row.bucket)))

function onRangeChange() {
  usageStore.load({ days: rangeDays.value })
}

function openPricing(model) {
  const price = pricingOf(model)
  pricingModel.value = model
  pricingForm.value = { input: price ? String(price.input) : '', output: price ? String(price.output) : '' }
  pricingError.value = ''
}

// 空着表示「这条不计价」；填了就必须是不小于 0 的数字，别把 NaN 送进金额计算
function parsePrice(text) {
  if (String(text).trim() === '') return null
  const value = Number(text)
  return Number.isFinite(value) && value >= 0 ? value : NaN
}

async function applyPricing(mutate) {
  const model = pricingModel.value
  const ok = await configStore.mutate((draft) => {
    if (!draft.settings.pricing) draft.settings.pricing = {}
    mutate(draft.settings.pricing, model)
  })
  if (!ok) {
    pricingError.value = configStore.error || '保存失败'
    return
  }
  pricingModel.value = null
  // 单价变了要重新聚合：费用是主进程按当前单价现算的
  usageStore.load()
}

async function savePricing() {
  const input = parsePrice(pricingForm.value.input)
  const output = parsePrice(pricingForm.value.output)
  if (Number.isNaN(input) || Number.isNaN(output)) {
    pricingError.value = '单价要填不小于 0 的数字'
    return
  }
  if (input === null && output === null) {
    pricingError.value = '输入与输出至少要填一个'
    return
  }
  await applyPricing((pricing, model) => {
    pricing[model] = { input: input ?? 0, output: output ?? 0 }
  })
}

async function clearPricing() {
  await applyPricing((pricing, model) => {
    delete pricing[model]
  })
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
              <th class="col-num">输出</th>
              <th class="col-num">费用</th>
              <th v-if="dimension === 'model'" class="col-actions"></th>
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
              <td class="col-num">{{ formatNumber(row.bucket.input) }}</td>
              <td class="col-num">{{ formatNumber(row.bucket.output) }}</td>
              <td class="col-num" :class="{ 'cell-muted': !row.bucket.priced }">
                {{ costText(row.bucket) }}<span v-if="isPartialCost(row.bucket)">*</span>
              </td>
              <td v-if="dimension === 'model'" class="col-actions">
                <BaseButton variant="ghost" size="sm" @click="openPricing(row.key)">
                  {{ pricingOf(row.key) ? '改单价' : '配单价' }}
                </BaseButton>
              </td>
            </tr>
          </tbody>
        </table>

        <p v-if="hasPartialCost" class="table-foot">
          * 该行里有请求用的模型没配单价，费用只算了配了单价的那部分
        </p>
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

    <BaseDialog
      :model-value="!!pricingModel"
      title="模型单价"
      width="420px"
      @update:model-value="pricingModel = null"
    >
      <p class="pricing-model"><code>{{ pricingModel }}</code></p>
      <p class="pricing-desc">
        单位是每百万 tokens，按供应商实际计费口径填。填完历史费用会用新单价重算，不用重跑统计。
      </p>

      <FormField label="输入单价" :error="pricingError">
        <BaseInput v-model="pricingForm.input" placeholder="例如 3" />
      </FormField>
      <FormField label="输出单价">
        <BaseInput v-model="pricingForm.output" placeholder="例如 15" />
      </FormField>

      <template #footer>
        <!-- 清除是破坏性操作，推到最左跟「取消/保存」隔开，防止顺手点错 -->
        <BaseButton
          v-if="pricingOf(pricingModel)"
          variant="danger"
          size="sm"
          class="footer-danger"
          :disabled="configStore.saving"
          @click="clearPricing"
        >
          清除单价
        </BaseButton>
        <BaseButton variant="secondary" size="sm" @click="pricingModel = null">取消</BaseButton>
        <BaseButton variant="primary" size="sm" :loading="configStore.saving" @click="savePricing">保存</BaseButton>
      </template>
    </BaseDialog>
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

/* 汇总：六个数并排，中间用竖线分开。分隔线比六个卡片省地方，
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

.stat-value.accent {
  color: var(--accent-text);
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

.col-actions {
  width: 80px;
  text-align: right;
  white-space: nowrap;
}

.table-foot {
  margin-top: var(--spacing-md);
  color: var(--text-muted);
  font-size: var(--font-size-xs);
}

/* ===== 单价弹窗 ===== */

.footer-danger {
  margin-right: auto;
}

.pricing-model {
  margin-bottom: var(--spacing-xs);
}

.pricing-model code {
  font-family: var(--font-family-mono);
  color: var(--text-primary);
  font-size: var(--font-size-base);
}

.pricing-desc {
  margin-bottom: var(--spacing-lg);
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  line-height: 1.6;
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
