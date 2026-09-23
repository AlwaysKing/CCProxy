<script setup>
/**
 * FormField - 表单字段包装器
 */
defineProps({
  label: {
    type: String,
    default: ''
  },
  required: {
    type: Boolean,
    default: false
  },
  error: {
    type: String,
    default: ''
  },
  hint: {
    type: String,
    default: ''
  }
})
</script>

<template>
  <div class="form-field" :class="{ 'has-error': error }">
    <div v-if="label || $slots['label-extra']" class="label-row">
      <label v-if="label" class="form-label">
        {{ label }}
        <span v-if="required" class="required">*</span>
      </label>
      <slot name="label-extra"></slot>
    </div>
    <div class="form-control">
      <slot></slot>
    </div>
    <p v-if="error" class="form-error">{{ error }}</p>
    <p v-else-if="hint" class="form-hint">{{ hint }}</p>
  </div>
</template>

<style scoped>
.form-field {
  margin-bottom: var(--spacing-lg);
}

/* label 与附加操作（如「拉取」按钮）同行，间距由 label-row 控制 */
.label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-sm);
  margin-bottom: var(--spacing-xs);
}

.form-label {
  display: block;
  font-size: var(--font-size-sm);
  font-weight: var(--font-weight-medium);
  color: var(--text-secondary);
}

.required {
  color: var(--color-error);
  margin-left: 2px;
}

.form-control {
  width: 100%;
}

.form-error {
  font-size: var(--font-size-xs);
  color: var(--color-error);
  margin-top: var(--spacing-xs);
}

.form-hint {
  font-size: var(--font-size-xs);
  color: var(--text-muted);
  margin-top: var(--spacing-xs);
}

.has-error :deep(input),
.has-error :deep(textarea),
.has-error :deep(select) {
  border-color: var(--color-error);
}

.has-error :deep(input:focus),
.has-error :deep(textarea:focus),
.has-error :deep(select:focus) {
  border-color: var(--color-error);
}
</style>
