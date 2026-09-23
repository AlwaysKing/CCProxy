<script setup>
/**
 * ConfirmDialog - 二次确认弹窗
 * 删除这类不可逆操作统一走它，省得每个页面自己拼一套 footer
 */
import BaseDialog from './BaseDialog.vue'
import BaseButton from './BaseButton.vue'

defineProps({
  modelValue: {
    type: Boolean,
    default: false
  },
  title: {
    type: String,
    default: '确认操作'
  },
  description: {
    type: String,
    default: ''
  },
  confirmText: {
    type: String,
    default: '确定'
  },
  danger: {
    type: Boolean,
    default: true
  },
  loading: {
    type: Boolean,
    default: false
  }
})

const emit = defineEmits(['update:modelValue', 'confirm'])
</script>

<template>
  <BaseDialog
    :model-value="modelValue"
    :title="title"
    width="420px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <p class="confirm-text">{{ description }}</p>

    <template #footer>
      <BaseButton variant="secondary" size="sm" @click="emit('update:modelValue', false)">取消</BaseButton>
      <BaseButton
        :variant="danger ? 'danger' : 'primary'"
        size="sm"
        :loading="loading"
        @click="emit('confirm')"
      >
        {{ confirmText }}
      </BaseButton>
    </template>
  </BaseDialog>
</template>

<style scoped>
.confirm-text {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-base);
  line-height: 1.6;
  word-break: break-word;
}
</style>
