<script setup>
// 供应商编辑弹窗：供应商共享名称、地址与 token，模型列表只分 Claude / OpenAI 两类。
import { computed, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { BaseButton, BaseDialog, BaseInput, FormField, IconButton } from '@/components/base'
import { newId } from '@/utils/id'

const props = defineProps({
  modelValue: {
    type: Boolean,
    default: false
  },
  provider: {
    type: Object,
    default: null
  }
})

const emit = defineEmits(['update:modelValue', 'saved'])

const configStore = useConfigStore()

const FORMATS = [
  {
    value: 'anthropic',
    label: 'Claude'
  },
  {
    value: 'openai',
    label: 'OpenAI'
  }
]

const form = ref({ name: '', baseUrl: '', token: '', modelGroups: [] })
const showToken = ref(false)
const error = ref('')
const fetching = ref(false)

const isEdit = computed(() => !!props.provider)

function formatLabel(apiFormat) {
  return FORMATS.find((item) => item.value === apiFormat)?.label || apiFormat
}

function modelListLabel(apiFormat) {
  if (apiFormat === 'anthropic') return 'Claude 模型列表'
  if (apiFormat === 'openai') return 'OpenAI 模型列表'
  return `${apiFormat} 模型列表`
}

function showModelAttrs(group) {
  return group.apiFormat === 'openai'
}

function emptyModel() {
  return { name: '', contextWindow: '', maxContextWindow: '', image: false, reasoning: true }
}

function emptyGroup(apiFormat) {
  return {
    id: `g_${apiFormat}`,
    apiFormat,
    models: []
  }
}

function createEmptyGroups() {
  return FORMATS.map((item) => emptyGroup(item.value))
}

function toRow(model) {
  const modalities = Array.isArray(model.inputModalities) ? model.inputModalities : []
  return {
    name: model.name,
    contextWindow: Number.isFinite(model.contextWindow) ? String(model.contextWindow) : '',
    maxContextWindow: Number.isFinite(model.maxContextWindow) ? String(model.maxContextWindow) : '',
    image: modalities.includes('image'),
    reasoning: model.reasoning === true
  }
}

function buildModels(group) {
  const models = []
  for (const row of group.models) {
    const name = row.name.trim()
    if (!name) continue
    if (!showModelAttrs(group)) {
      models.push({ name })
      continue
    }
    const model = {
      name,
      inputModalities: row.image ? ['text', 'image'] : ['text'],
      reasoning: row.reasoning === true
    }
    if (String(row.contextWindow).trim() !== '') model.contextWindow = Number(row.contextWindow)
    if (String(row.maxContextWindow).trim() !== '') model.maxContextWindow = Number(row.maxContextWindow)
    models.push(model)
  }
  return models
}

watch(
  () => props.modelValue,
  (visible) => {
    if (!visible) return
    error.value = ''
    fetching.value = false
    showToken.value = false
    const existing = new Map((props.provider?.modelGroups ?? []).map((group) => [group.apiFormat, group]))
    form.value = {
      name: props.provider?.name ?? '',
      baseUrl: props.provider?.baseUrl ?? '',
      token: props.provider?.token ?? '',
      modelGroups: FORMATS.map((item) => {
        const group = existing.get(item.value)
        return group
          ? {
              id: group.id || `g_${item.value}`,
              apiFormat: item.value,
              models: (group.models ?? []).map(toRow)
            }
          : emptyGroup(item.value)
      })
    }
  }
)

function groupIsConfigured(group) {
  return group.models.some((row) => row.name.trim().length > 0)
}

function validateGroup(group) {
  const label = formatLabel(group.apiFormat)
  if (!groupIsConfigured(group)) return ''
  const names = group.models.map((row) => row.name.trim()).filter(Boolean)
  if (names.length === 0) return `${label} 至少配置一个模型`
  const seen = new Set()
  const duplicated = new Set()
  for (const name of names) {
    if (seen.has(name)) duplicated.add(name)
    seen.add(name)
  }
  if (duplicated.size > 0) return `${label} 的模型名不能重复：${[...duplicated].join('、')}`

  if (!showModelAttrs(group)) return ''
  for (const row of group.models) {
    const model = row.name.trim()
    if (!model) continue
    for (const [field, label] of [['contextWindow', '上下文窗口'], ['maxContextWindow', '最大上下文窗口']]) {
      const text = String(row[field]).trim()
      if (text === '') continue
      if (!/^\d+$/.test(text) || Number(text) <= 0) return `模型 ${model} 的${label}要填正整数`
    }
  }
  return ''
}

function validate() {
  if (!form.value.name.trim()) return '名称不能为空'
  const baseUrl = form.value.baseUrl.trim()
  if (!baseUrl) return '上游地址不能为空'
  if (!/^https?:\/\//.test(baseUrl)) return '上游地址要以 http:// 或 https:// 开头'
  for (const group of form.value.modelGroups) {
    const message = validateGroup(group)
    if (message) return message
  }
  if (!form.value.modelGroups.some(groupIsConfigured)) return '至少配置一个模型组'
  return ''
}

function addModel(group) {
  group.models.push(emptyModel())
}

function removeModel(group, index) {
  group.models.splice(index, 1)
}

async function fetchGroupModels(group) {
  const baseUrl = form.value.baseUrl.trim()
  if (!/^https?:\/\//.test(baseUrl)) {
    throw new Error('上游地址要以 http:// 或 https:// 开头')
  }
  const result = await window.electronAPI.fetchProviderModels({
    apiFormat: group.apiFormat,
    baseUrl,
    token: form.value.token.trim()
  })
  if (!result || !result.ok) {
    throw new Error((result && result.error) || '拉取失败')
  }
  if (result.models.length === 0) {
    throw new Error('上游没有返回任何模型')
  }
  const existing = new Set(group.models.map((row) => row.name.trim()).filter(Boolean))
  group.models = group.models.filter((row) => row.name.trim())
  for (const name of result.models) {
    if (existing.has(name)) continue
    group.models.push(emptyModel())
    group.models[group.models.length - 1].name = name
  }
}

async function onFetchModels() {
  const baseUrl = form.value.baseUrl.trim()
  if (!/^https?:\/\//.test(baseUrl)) {
    error.value = '拉取前先把上游地址填对（要以 http:// 或 https:// 开头）'
    return
  }
  fetching.value = true
  error.value = ''
  const failures = []
  try {
    for (const group of form.value.modelGroups) {
      try {
        await fetchGroupModels(group)
      } catch (cause) {
        failures.push(`${modelListLabel(group.apiFormat)}：${cause.message || '拉取失败'}`)
      }
    }
    if (failures.length) error.value = failures.join('；')
  } finally {
    fetching.value = false
  }
}

async function onConfirm() {
  error.value = validate()
  if (error.value) return

  const saved = await configStore.upsertProvider({
    id: props.provider?.id ?? newId('p'),
    name: form.value.name.trim(),
    enabled: props.provider?.enabled ?? true,
    baseUrl: form.value.baseUrl.trim(),
    token: form.value.token.trim(),
    modelGroups: form.value.modelGroups
      .filter(groupIsConfigured)
      .map((group) => ({
        id: group.id,
        apiFormat: group.apiFormat,
        models: buildModels(group)
      }))
  })
  if (!saved) {
    error.value = configStore.error || '保存失败'
    return
  }
  emit('saved')
  emit('update:modelValue', false)
}
</script>

<template>
  <BaseDialog
    :model-value="modelValue"
    :title="isEdit ? '编辑供应商' : '添加供应商'"
    width="680px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <FormField label="名称" required>
      <BaseInput v-model="form.name" placeholder="智谱" />
    </FormField>

    <FormField label="上游地址" required hint="例：https://open.bigmodel.cn/api/anthropic；转发时会把请求路径接在它后面">
      <BaseInput v-model="form.baseUrl" placeholder="https://open.bigmodel.cn/api/anthropic" />
    </FormField>

    <FormField label="token" hint="按上游格式注入：Anthropic 发 x-api-key，OpenAI 一族发 Authorization: Bearer">
      <BaseInput v-model="form.token" :type="showToken ? 'text' : 'password'" placeholder="sk-...">
        <template #suffix>
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
        </template>
      </BaseInput>
    </FormField>

    <section class="models-section">
      <header class="models-head">
        <div>
          <h3 class="models-title">模型列表</h3>
          <p class="models-desc">
            点击一次拉取，会分别按 Claude 与 OpenAI 两种协议请求上游并更新对应模型列表。
          </p>
        </div>
        <BaseButton
          variant="secondary"
          size="sm"
          :loading="fetching"
          :disabled="!form.baseUrl.trim()"
          @click="onFetchModels"
        >拉取</BaseButton>
      </header>

      <div class="group-list">
        <section v-for="group in form.modelGroups" :key="group.apiFormat" class="model-group">
          <FormField
            :label="modelListLabel(group.apiFormat)"
            :required="groupIsConfigured(group)"
          >
            <div class="model-list">
              <div v-for="(row, index) in group.models" :key="index" class="model-row">
                <BaseInput v-model="row.name" placeholder="模型名" />

                <template v-if="showModelAttrs(group)">
                  <BaseInput
                    v-model="row.contextWindow"
                    size="sm"
                    class="window-input"
                    placeholder="上下文窗口"
                    title="上下文窗口（tokens）"
                  />
                  <BaseInput
                    v-model="row.maxContextWindow"
                    size="sm"
                    class="window-input"
                    placeholder="最大窗口"
                    title="最大上下文窗口（tokens）"
                  />
                  <label class="cap"><input type="checkbox" checked disabled /> 文本</label>
                  <label class="cap"><input v-model="row.image" type="checkbox" /> 图片</label>
                  <label class="cap"><input v-model="row.reasoning" type="checkbox" /> 推理</label>
                </template>

                <IconButton size="sm" class="row-remove" title="删除该模型" @click="removeModel(group, index)">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                </IconButton>
              </div>
            </div>

            <BaseButton variant="secondary" size="sm" @click="addModel(group)">+ 添加模型</BaseButton>
          </FormField>
        </section>
      </div>
    </section>

    <p v-if="error" class="dialog-error">{{ error }}</p>

    <template #footer>
      <BaseButton variant="secondary" size="sm" @click="emit('update:modelValue', false)">取消</BaseButton>
      <BaseButton size="sm" :loading="configStore.saving" @click="onConfirm">保存</BaseButton>
    </template>
  </BaseDialog>
</template>

<style scoped>
.models-section {
  margin-top: var(--spacing-xl);
  padding: var(--spacing-md);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: var(--bg-secondary);
}

.models-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--spacing-lg);
  margin-bottom: var(--spacing-lg);
}

.models-title {
  margin: 0;
  color: var(--text-primary);
  font-size: var(--font-size-md);
  font-weight: var(--font-weight-medium);
}

.models-desc {
  margin: 2px 0 0;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}

.group-list {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-lg);
}

.model-group {
  min-width: 0;
}

.model-list {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-xs);
  margin-bottom: var(--spacing-sm);
}

.model-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--spacing-xs) var(--spacing-sm);
  padding: var(--spacing-xs) var(--spacing-sm);
  background-color: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

.model-row :deep(.base-input-wrapper) {
  flex: 1 1 140px;
}

.model-row .window-input {
  flex: 0 0 104px;
}

.cap {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  white-space: nowrap;
  cursor: pointer;
}

.cap input:disabled {
  cursor: default;
}

.row-remove {
  margin-left: auto;
}

.dialog-error {
  margin: var(--spacing-md) 0 0;
  color: var(--color-error);
  font-size: var(--font-size-sm);
}
</style>
