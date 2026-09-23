<script setup>
/**
 * Badge - 徽章/标签组件
 */
defineProps({
  variant: {
    type: String,
    default: 'default',
    validator: (v) => ['default', 'primary', 'success', 'error', 'warning', 'info'].includes(v)
  },
  size: {
    type: String,
    default: 'md',
    validator: (v) => ['sm', 'md', 'lg'].includes(v)
  }
})
</script>

<template>
  <span class="badge" :class="[`badge-${variant}`, `badge-${size}`]">
    <slot></slot>
  </span>
</template>

<style scoped>
.badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-weight: var(--font-weight-medium);
  border-radius: var(--radius-sm);
  white-space: nowrap;
}

/* 尺寸 */
.badge-sm {
  padding: 2px 6px;
  font-size: var(--font-size-xs);
}

.badge-md {
  padding: 3px 8px;
  font-size: var(--font-size-sm);
}

.badge-lg {
  padding: 4px 10px;
  font-size: var(--font-size-base);
}

/* 变体 */
.badge-default {
  background-color: var(--bg-tertiary);
  color: var(--text-secondary);
}

/* 文字用 --accent-text 而不是 --accent-primary：跟下拉选中项同一条规矩——
   橙色压在自己的淡色底纹上会掉到 4.2:1，亮一档才够读 */
.badge-primary {
  background-color: var(--accent-light);
  color: var(--accent-text);
}

.badge-success {
  background-color: var(--color-success-bg);
  color: var(--color-success);
}

.badge-error {
  background-color: var(--color-error-bg);
  color: var(--color-error);
}

.badge-warning {
  background-color: var(--color-warning-bg);
  color: var(--color-warning);
}

.badge-info {
  background-color: var(--color-info-bg);
  color: var(--color-info);
}
</style>
