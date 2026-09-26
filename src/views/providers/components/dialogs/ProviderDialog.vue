<script setup>
// 供应商编辑弹窗：名称、上游地址、token、模型配置列表。
// 模型是配置条目而不是纯文本：OpenAI 协议的模型可配上下文窗口、输入模态与推理能力
// （字段口径参考 codex 的 models.json），Anthropic 协议只出模型名——它那边这些属性用不上，
// 所以属性一行只在 OpenAI 协议下显示。
// 模型名是映射弹窗「真实模型」下拉的数据源，至少留一个。
import { computed, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { BaseButton, BaseDialog, BaseInput, FormField, IconButton } from '@/components/base'
import { newId } from '@/utils/id'

const props = defineProps({
  modelValue: {
    type: Boolean,
    default: false
  },
  protocol: {
    type: Object,
    required: true
  },
  provider: {
    type: Object,
    default: null
  }
})

const emit = defineEmits(['update:modelValue', 'saved'])

const configStore = useConfigStore()

const form = ref({ name: '', baseUrl: '', token: '', models: [] })
const showToken = ref(false)
const error = ref('')
const fetching = ref(false)

const isEdit = computed(() => !!props.provider)

// 只有 OpenAI 协议的模型显示窗口与模态配置
const showModelAttrs = computed(() => props.protocol.id === 'openai')

function emptyModel(name = '') {
  return { name, contextWindow: '', maxContextWindow: '', image: false, reasoning: false }
}

// 配置条目 → 表单行：窗口空值统一成空串（留空就是没配），模态拆成勾选
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

watch(
  () => props.modelValue,
  (visible) => {
    if (!visible) return
    error.value = ''
    showToken.value = false
    form.value = props.provider
      ? {
          name: props.provider.name,
          baseUrl: props.provider.baseUrl,
          token: props.provider.token,
          models: props.provider.models.map(toRow)
        }
      : { name: '', baseUrl: '', token: '', models: [emptyModel()] }
  }
)

// 表单行 → 配置条目：名称必填，没名字的行直接丢掉；窗口留空不当成 0 存；
// 文本是模型的底线能力固定带上，图片与推理按勾选。
// 勾选框只有两态，所以 OpenAI 的模型一律写出 reasoning 布尔值——没勾就是显式 false，
// 这里不存在「未配置」态；Anthropic 侧整个属性都不写
function buildModels() {
  const models = []
  for (const row of form.value.models) {
    const name = row.name.trim()
    if (!name) continue
    if (!showModelAttrs.value) {
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

function validate() {
  const { name, baseUrl } = form.value
  if (!name.trim()) return '名称不能为空'
  const url = baseUrl.trim()
  if (!url) return '上游地址不能为空'
  if (!/^https?:\/\//.test(url)) return '上游地址要以 http:// 或 https:// 开头'
  const names = form.value.models.map((row) => row.name.trim()).filter(Boolean)
  if (names.length === 0) return '至少配置一个模型'
  // 重名的报出来是哪几个：列表长了光说「不能重复」得自己挨个比对
  const seen = new Set()
  const duplicated = new Set()
  for (const name of names) {
    if (seen.has(name)) duplicated.add(name)
    seen.add(name)
  }
  if (duplicated.size > 0) return `模型名不能重复：${[...duplicated].join('、')}`
  // 窗口只在 OpenAI 协议下可填，不是正整数的挡在保存前，别把 NaN 写进配置。
  // 没填名字的行保存时会被丢掉，不在这里卡它
  for (const row of form.value.models) {
    const model = row.name.trim()
    if (!model) continue
    for (const [field, label] of [['contextWindow', '上下文窗口'], ['maxContextWindow', '最大上下文窗口']]) {
      const text = String(row[field]).trim()
      if (text === '') continue
      // 与配置层同一口径：只认纯数字串，1e5、小数点这些写法不收
      if (!/^\d+$/.test(text) || Number(text) <= 0) return `模型 ${model} 的${label}要填正整数`
    }
  }
  return ''
}

function addModel() {
  form.value.models.push(emptyModel())
}

function removeModel(index) {
  form.value.models.splice(index, 1)
}

// 按上方填写的上游地址与 token 拉取上游模型列表，合并进模型配置列表
async function onFetchModels() {
  const baseUrl = form.value.baseUrl.trim()
  if (!/^https?:\/\//.test(baseUrl)) {
    error.value = '拉取前先把上游地址填对（要以 http:// 或 https:// 开头）'
    return
  }
  fetching.value = true
  error.value = ''
  try {
    const result = await window.electronAPI.fetchProviderModels({
      protocolId: props.protocol.id,
      baseUrl,
      token: form.value.token.trim()
    })
    if (!result || !result.ok) {
      error.value = (result && result.error) || '拉取失败'
      return
    }
    if (result.models.length === 0) {
      error.value = '上游没有返回任何模型'
      return
    }
    // 合并而不是覆盖：同名的保留已填的属性，新模型补空行；上游没报而列表里已有的
    // （例如网关不回显的内部别名）原样留着，别当成删除
    const existing = new Set(form.value.models.map((row) => row.name.trim()).filter(Boolean))
    form.value.models = form.value.models.filter((row) => row.name.trim())
    for (const name of result.models) {
      if (existing.has(name)) continue
      form.value.models.push(emptyModel(name))
    }
  } finally {
    fetching.value = false
  }
}

async function onConfirm() {
  error.value = validate()
  if (error.value) return

  const saved = await configStore.upsertProvider(props.protocol.id, {
    id: props.provider?.id ?? newId('p'),
    name: form.value.name.trim(),
    enabled: props.provider?.enabled ?? true,
    baseUrl: form.value.baseUrl.trim(),
    token: form.value.token.trim(),
    models: buildModels()
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

    <FormField label="token" hint="按上游协议注入：Anthropic 发 x-api-key，OpenAI 发 Authorization: Bearer">
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

    <FormField
      label="模型列表"
      required
      :hint="showModelAttrs ? '上下文窗口按供应商实际口径填（tokens）；留空表示未配置' : '模型名会出现在映射弹窗的「真实模型」下拉里'"
    >
      <template #label-extra>
        <BaseButton
          variant="secondary"
          size="sm"
          :loading="fetching"
          :disabled="!form.baseUrl.trim()"
          title="按上方填写的上游地址和 token 拉取上游支持的模型，合并进下方列表"
          @click="onFetchModels"
        >拉取</BaseButton>
      </template>

      <div class="model-list">
        <div v-for="(row, index) in form.models" :key="index" class="model-row">
          <BaseInput v-model="row.name" placeholder="模型名，例如 gpt-5.5" />

          <template v-if="showModelAttrs">
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
            <!-- 文本是模型的底线能力，固定支持不可取消 -->
            <label class="cap"><input type="checkbox" checked disabled /> 文本</label>
            <label class="cap"><input v-model="row.image" type="checkbox" /> 图片</label>
            <label class="cap"><input v-model="row.reasoning" type="checkbox" /> 推理</label>
          </template>

          <IconButton size="sm" class="row-remove" title="删除该模型" @click="removeModel(index)">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </IconButton>
        </div>
      </div>

      <BaseButton variant="secondary" size="sm" @click="addModel">+ 添加模型</BaseButton>
    </FormField>

    <p v-if="error" class="dialog-error">{{ error }}</p>

    <template #footer>
      <BaseButton variant="secondary" size="sm" @click="emit('update:modelValue', false)">取消</BaseButton>
      <BaseButton size="sm" :loading="configStore.saving" @click="onConfirm">保存</BaseButton>
    </template>
  </BaseDialog>
</template>

<style scoped>
/* 模型配置列表：一个模型一行——名称、窗口、勾选、删除。
   一行放不下（窄窗口）时按 flex-wrap 折行，勾选项跟着窗口走，删除键始终贴行尾 */
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
  background-color: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

/* 模型名吃掉剩余宽度：窗口是定长数字，给宽了只会把勾选挤到下一行 */
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
