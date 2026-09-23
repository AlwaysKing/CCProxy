<script setup>
// 堆叠条 + 图例：用量页有两处要用（协议两端各占多少、模型各占多少）。
// 手写不引图表库——这里的诉求只有「谁占得多」一件事，为它拉一个库进来不值当。
// 超过 6 类并成「其他」：再多色相肉眼已经分不出来，硬画只会变成一条花边
import { computed } from 'vue'
import { formatNumber, formatTokens } from '@/utils/format'

const MAX_SEGMENTS = 6

const props = defineProps({
  title: {
    type: String,
    required: true
  },
  // [{ key, label, value }]，调用方按顺序给（多的在前），配色由这里按位次分配
  segments: {
    type: Array,
    default: () => []
  }
})

const total = computed(() => props.segments.reduce((sum, item) => sum + item.value, 0))

const visible = computed(() => {
  const positive = props.segments.filter((item) => item.value > 0)
  const head = positive.slice(0, MAX_SEGMENTS).map((item, index) => ({ ...item, color: `var(--chart-${index + 1})` }))
  if (positive.length <= MAX_SEGMENTS) return head
  const rest = positive.slice(MAX_SEGMENTS).reduce((sum, item) => sum + item.value, 0)
  return [...head, { key: '__rest', label: `其他 ${positive.length - MAX_SEGMENTS} 项`, value: rest, color: 'var(--chart-rest)' }]
})

function percent(value) {
  if (!total.value) return 0
  return (value / total.value) * 100
}
</script>

<template>
  <div class="usage-bar">
    <div class="bar-head">
      <span class="bar-title">{{ title }}</span>
      <span class="bar-total">{{ formatTokens(total) }} tokens</span>
    </div>

    <div v-if="visible.length" class="bar-track">
      <span
        v-for="item in visible"
        :key="item.key"
        class="bar-segment"
        :style="{ width: `${percent(item.value)}%`, backgroundColor: item.color }"
        :title="`${item.label}：${formatNumber(item.value)} tokens（${percent(item.value).toFixed(1)}%）`"
      ></span>
    </div>
    <p v-else class="bar-empty">这段时间没有消耗记录</p>

    <ul v-if="visible.length" class="bar-legend">
      <li v-for="item in visible" :key="item.key" class="legend-item">
        <span class="legend-dot" :style="{ backgroundColor: item.color }"></span>
        <span class="legend-label">{{ item.label }}</span>
        <span class="legend-value">{{ percent(item.value).toFixed(1) }}%</span>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.usage-bar {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-sm);
}

.bar-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--spacing-md);
}

.bar-title {
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  font-weight: var(--font-weight-medium);
}

.bar-total {
  color: var(--text-muted);
  font-size: var(--font-size-xs);
  font-family: var(--font-family-mono);
}

/* 条子本身只负责「形」：圆角、高度、段与段之间的 2px 缝。
   缝用间距而不是描边——描边在窄段上会盖掉颜色本身 */
.bar-track {
  display: flex;
  align-items: stretch;
  gap: 2px;
  height: 10px;
  border-radius: var(--radius-full);
  overflow: hidden;
  background-color: var(--bg-tertiary);
}

.bar-segment {
  display: block;
  height: 100%;
  transition: width var(--transition-normal);
}

.bar-empty {
  color: var(--text-muted);
  font-size: var(--font-size-sm);
}

.bar-legend {
  display: flex;
  flex-wrap: wrap;
  gap: var(--spacing-xs) var(--spacing-lg);
  list-style: none;
}

.legend-item {
  display: flex;
  align-items: center;
  gap: var(--spacing-xs);
  font-size: var(--font-size-sm);
}

.legend-dot {
  width: 8px;
  height: 8px;
  border-radius: var(--radius-full);
}

.legend-label {
  color: var(--text-secondary);
}

.legend-value {
  color: var(--text-primary);
  font-family: var(--font-family-mono);
}
</style>
