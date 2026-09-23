<script setup>
/**
 * BaseTabs - 分段式标签切换
 * 选项数据驱动、单一选中值：顶部协议切换用它，后续日志页按协议过滤也能复用
 */
defineProps({
  modelValue: {
    type: [String, Number],
    default: null
  },
  items: {
    type: Array,
    default: () => []
  },
  size: {
    type: String,
    default: 'md',
    validator: (v) => ['sm', 'md'].includes(v)
  }
})

const emit = defineEmits(['update:modelValue', 'change'])

function select(value) {
  emit('update:modelValue', value)
  emit('change', value)
}
</script>

<template>
  <div class="base-tabs" :class="`tabs-${size}`" role="tablist">
    <button
      v-for="item in items"
      :key="item.value"
      type="button"
      role="tab"
      class="tab-item"
      :class="{ active: item.value === modelValue }"
      :title="item.hint || ''"
      @click="select(item.value)"
    >
      {{ item.label }}
    </button>
  </div>
</template>

<style scoped>
.base-tabs {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 2px;
  background-color: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

.tab-item {
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-family: inherit;
  font-weight: var(--font-weight-medium);
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.tabs-sm .tab-item {
  padding: 4px 10px;
  font-size: var(--font-size-sm);
}

.tabs-md .tab-item {
  padding: 6px 14px;
  font-size: var(--font-size-base);
}

.tab-item:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

.tab-item.active {
  background-color: var(--accent-primary);
  color: var(--text-inverse);
}
</style>
