<script setup>
// 供应商编辑弹窗：名称、上游地址、token、模型列表。
// 模型列表按行输入（也容忍逗号分隔），至少留一个——它是映射弹窗「真实模型」下拉的数据源，
// 没配模型的供应商在映射里选不出模型。
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

const form = ref({ name: '', baseUrl: '', token: '', modelsText: '' })
const showToken = ref(false)
const error = ref('')
const fetching = ref(false)

const isEdit = computed(() => !!props.provider)

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
          modelsText: props.provider.models.join('\n')
        }
      : { name: '', baseUrl: '', token: '', modelsText: '' }
  }
)

function parseModels() {
  return form.value.modelsText
    .split(/[\n,]/)
    .map((model) => model.trim())
    .filter(Boolean)
}

function validate() {
  const { name, baseUrl } = form.value
  if (!name.trim()) return '名称不能为空'
  const url = baseUrl.trim()
  if (!url) return '上游地址不能为空'
  if (!/^https?:\/\//.test(url)) return '上游地址要以 http:// 或 https:// 开头'
  if (parseModels().length === 0) return '至少配置一个模型'
  return ''
}

// 按上方填写的上游地址与 token 拉取上游模型列表，直接覆盖模型列表输入框
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
    form.value.modelsText = result.models.join('\n')
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
    models: parseModels()
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
    width="520px"
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

    <FormField label="模型列表" required hint="每行一个模型名（也支持逗号分隔），例如 glm-4.6">
      <template #label-extra>
        <BaseButton
          variant="secondary"
          size="sm"
          :loading="fetching"
          :disabled="!form.baseUrl.trim()"
          title="按上方填写的上游地址和 token 拉取上游支持的模型，覆盖下方列表"
          @click="onFetchModels"
        >拉取</BaseButton>
      </template>
      <textarea
        v-model="form.modelsText"
        class="models-textarea"
        rows="4"
        placeholder="glm-4.6&#10;glm-4.5-air"
      ></textarea>
    </FormField>

    <p v-if="error" class="dialog-error">{{ error }}</p>

    <template #footer>
      <BaseButton variant="secondary" size="sm" @click="emit('update:modelValue', false)">取消</BaseButton>
      <BaseButton size="sm" :loading="configStore.saving" @click="onConfirm">保存</BaseButton>
    </template>
  </BaseDialog>
</template>

<style scoped>
/* 与 BaseInput 同一套外观，模型列表要能多行输入 */
.models-textarea {
  width: 100%;
  padding: 8px 12px;
  background-color: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  color: var(--text-primary);
  font-family: var(--font-family-mono);
  font-size: var(--font-size-sm);
  line-height: 1.6;
  resize: vertical;
  outline: none;
  transition: border-color var(--transition-fast);
}

.models-textarea:focus {
  border-color: var(--accent-primary);
}

.models-textarea::placeholder {
  color: var(--text-muted);
}

.dialog-error {
  margin: var(--spacing-md) 0 0;
  color: var(--color-error);
  font-size: var(--font-size-sm);
}
</style>
