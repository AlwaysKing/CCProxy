<script setup>
// 行内项目同步状态：图标负责快速识别，Teleport 提示层负责完整原因。
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'

const props = defineProps({
  client: {
    type: String,
    required: true
  },
  status: {
    type: Object,
    default: null
  }
})

const STATUS_META = {
  ok: { label: '已同步', glyph: '✓', severity: 'success' },
  binding_invalid: { label: '绑定失效', glyph: '!', severity: 'warning' },
  project_missing: { label: '目录不存在', glyph: '!', severity: 'warning' },
  restart_required: { label: '待重启 Codex', glyph: '!', severity: 'warning' },
  requires_project_trust: { label: '需信任项目', glyph: '!', severity: 'warning' },
  config_unreadable: { label: '配置不可读', glyph: '×', severity: 'error' },
  write_failed: { label: '写入失败', glyph: '×', severity: 'error' },
  sync_disabled: { label: '同步未启用', glyph: '?', severity: 'neutral' }
}

const iconRef = ref(null)
const tooltipRef = ref(null)
const visible = ref(false)
const tooltipStyle = ref({})

const clientLabel = computed(() => (props.client === 'codex' ? 'Codex' : 'Claude'))
const meta = computed(
  () =>
    STATUS_META[props.status?.status] || {
      label: '未知状态',
      glyph: '?',
      severity: 'neutral'
    }
)
const accessibleLabel = computed(() => {
  const message = props.status?.message?.trim()
  return `${clientLabel.value} 同步状态：${meta.value.label}${message ? `，${message}` : ''}`
})

function updatePosition() {
  if (!iconRef.value || !tooltipRef.value) return
  const anchor = iconRef.value.getBoundingClientRect()
  const width = tooltipRef.value.offsetWidth
  const height = tooltipRef.value.offsetHeight
  const padding = 8
  const gap = 8
  const left = Math.min(
    Math.max(padding, anchor.left + anchor.width / 2 - width / 2),
    window.innerWidth - width - padding
  )
  const above = anchor.top - height - gap
  const top =
    above >= padding
      ? above
      : Math.min(anchor.bottom + gap, window.innerHeight - height - padding)
  tooltipStyle.value = {
    left: `${left}px`,
    top: `${Math.max(padding, top)}px`,
    width: `${width}px`
  }
}

async function show() {
  visible.value = true
  await nextTick()
  updatePosition()
}

function hide() {
  visible.value = false
}

onMounted(() => {
  window.addEventListener('scroll', hide, true)
  window.addEventListener('resize', hide)
})

onBeforeUnmount(() => {
  window.removeEventListener('scroll', hide, true)
  window.removeEventListener('resize', hide)
})
</script>

<template>
  <span
    ref="iconRef"
    class="project-status"
    :class="`is-${meta.severity}`"
    role="img"
    tabindex="0"
    :aria-label="accessibleLabel"
    @mouseenter="show"
    @mouseleave="hide"
    @focus="show"
    @blur="hide"
  >
    <span aria-hidden="true">{{ meta.glyph }}</span>
  </span>

  <Teleport to="body">
    <div
      v-if="visible"
      ref="tooltipRef"
      class="project-status-tooltip"
      role="tooltip"
      :style="tooltipStyle"
    >
      <strong>{{ clientLabel }} · {{ meta.label }}</strong>
      <span v-if="status?.message">{{ status.message }}</span>
    </div>
  </Teleport>
</template>

<style scoped>
.project-status {
  display: inline-grid;
  flex: 0 0 20px;
  place-items: center;
  width: 20px;
  height: 20px;
  border: 1px solid currentColor;
  border-radius: 50%;
  font-size: 12px;
  font-weight: var(--font-weight-semibold);
  line-height: 1;
  cursor: help;
}

.project-status.is-success {
  color: var(--color-success);
  background: var(--color-success-bg);
}

.project-status.is-warning {
  color: var(--color-warning);
  background: var(--color-warning-bg);
}

.project-status.is-error {
  color: var(--color-error);
  background: var(--color-error-bg);
}

.project-status.is-neutral {
  color: var(--text-muted);
  background: var(--bg-tertiary);
}

.project-status-tooltip {
  position: fixed;
  z-index: var(--z-tooltip);
  display: flex;
  flex-direction: column;
  gap: 3px;
  max-width: min(320px, calc(100vw - 32px));
  padding: 8px 10px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  box-shadow: var(--shadow-md);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  line-height: var(--line-height-normal);
  overflow-wrap: anywhere;
  pointer-events: none;
  white-space: normal;
}

.project-status-tooltip strong {
  font-weight: var(--font-weight-semibold);
}

.project-status-tooltip span {
  color: var(--text-secondary);
}
</style>
