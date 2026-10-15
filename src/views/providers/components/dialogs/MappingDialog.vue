<script setup>
// 映射编辑弹窗：客户端模型名 → 供应商 + 真实模型。
// 真实模型从所选供应商的模型列表里下拉选；客户端模型可留空，留空时自动取「供应商名-真实模型」。
// 校验收在这里：必须选供应商与真实模型、同一个客户端模型不能配两条
//（配置层只按 id 去重，重名的两条谁命中全看查表顺序，属于说不清的配置，直接挡掉）。
import { computed, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { AppSelect, BaseButton, BaseDialog, BaseInput, FormField } from '@/components/base'
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
  mapping: {
    type: Object,
    default: null
  }
})

const emit = defineEmits(['update:modelValue', 'saved'])

const configStore = useConfigStore()

const form = ref({ clientModel: '', providerId: '', upstreamProtocol: '', realModel: '' })
const error = ref('')

const isEdit = computed(() => !!props.mapping)
// 供应商来自全局池；真实模型必须从映射明确选择的模型组里取。
const providerOptions = computed(() =>
  configStore.providers.map((provider) => ({ value: provider.id, label: provider.name }))
)
const selectedProvider = computed(
  () => configStore.providers.find((provider) => provider.id === form.value.providerId) || null
)

function formatLabel(apiFormat) {
  if (apiFormat === 'anthropic') return 'Claude'
  if (apiFormat === 'openai') return 'OpenAI'
  return apiFormat
}

const upstreamOptions = computed(() => {
  const options = []
  if (selectedProvider.value?.modelGroups.some((group) => group.apiFormat === 'anthropic')) {
    options.push({ value: 'anthropic', label: 'Claude / Anthropic Messages' })
  }
  if (selectedProvider.value?.modelGroups.some((group) => group.apiFormat === 'openai')) {
    options.push({ value: 'openai_chat', label: 'OpenAI Chat' })
    options.push({ value: 'openai_responses', label: 'OpenAI Responses' })
  }
  return options
})
const selectedGroup = computed(
  () => {
    if (!form.value.upstreamProtocol) return null
    const format = form.value.upstreamProtocol === 'anthropic' ? 'anthropic' : 'openai'
    return selectedProvider.value?.modelGroups.find((group) => group.apiFormat === format) || null
  }
)

const conversionSupported = computed(() => {
  const client = props.protocol.id
  const upstream = form.value.upstreamProtocol
  if (!upstream || client === upstream) return true
  return [
    ['anthropic', 'openai_chat'],
    ['anthropic', 'openai_responses'],
    ['openai_responses', 'openai_chat'],
    ['openai_responses', 'anthropic']
  ].some(([from, to]) => from === client && to === upstream)
})

const conversionMessage = computed(() => {
  const client = props.protocol.id
  const upstream = form.value.upstreamProtocol
  if (!upstream) return ''
  if (client === upstream) return '客户端与上游协议相同：直接替换模型名和凭据后转发。'
  if (conversionSupported.value) return '客户端与上游协议不同：将调用现有协议转换模块。'
  return '当前尚未支持这条协议转换，请更换上游协议或供应商。'
})

function modelNames(group) {
  return (group?.models ?? []).map((model) => model.name)
}

function firstUpstreamProtocol(provider) {
  if (!provider?.modelGroups?.length) return ''
  return provider.modelGroups[0].apiFormat === 'anthropic' ? 'anthropic' : 'openai_chat'
}

function groupForUpstream(provider, upstreamProtocol) {
  const format = upstreamProtocol === 'anthropic' ? 'anthropic' : 'openai'
  return provider?.modelGroups.find((group) => group.apiFormat === format) || null
}

const modelOptions = computed(() => {
  const models = modelNames(selectedGroup.value)
  const options = models.map((model) => ({ value: model, label: model }))
  // 编辑回填的旧值可能已不在该供应商的模型列表里（后来改过列表）：保留成一项并标出来，
  // 不然下拉显示成空的，用户一保存这个值就没了
  const current = form.value.realModel
  if (current && !models.includes(current)) {
    options.unshift({ value: current, label: `${current}（不在该模型组中）` })
  }
  return options
})

watch(
  () => props.modelValue,
  (visible) => {
    if (!visible) return
    error.value = ''
    // 新增时供应商、上游协议和模型都默认选第一项：只有一个供应商的常见场景下能少点两次
    const first = configStore.providers[0]
    const firstUpstream = firstUpstreamProtocol(first)
    const firstGroup = groupForUpstream(first, firstUpstream)
    form.value = props.mapping
      ? {
          clientModel: props.mapping.clientModel,
          providerId: props.mapping.providerId,
          upstreamProtocol: props.mapping.upstreamProtocol,
          realModel: props.mapping.realModel
        }
      : {
          clientModel: '',
          providerId: first?.id ?? '',
          upstreamProtocol: firstUpstream,
          realModel: modelNames(firstGroup)[0] ?? ''
        }
  }
)

// 换供应商后旧上游协议与模型都可能失效：换成新供应商的第一个协议与模型。
watch(
  () => form.value.providerId,
  (id, prevId) => {
    if (!id || !prevId || id === prevId) return
    const provider = configStore.providers.find((item) => item.id === id)
    const upstream = firstUpstreamProtocol(provider)
    form.value.upstreamProtocol = upstream
    const models = modelNames(groupForUpstream(provider, upstream))
    if (!models.includes(form.value.realModel)) form.value.realModel = models[0] ?? ''
  }
)

// 换上游协议后只切换模型列表；OpenAI Chat / Responses 共用同一个模型组。
watch(
  () => form.value.upstreamProtocol,
  (upstream, prevUpstream) => {
    if (!upstream || !prevUpstream || upstream === prevUpstream) return
    const group = groupForUpstream(selectedProvider.value, upstream)
    const models = modelNames(group)
    if (!models.includes(form.value.realModel)) form.value.realModel = models[0] ?? ''
  }
)

function resolveClientModel(provider, realModel) {
  const typed = form.value.clientModel.trim()
  return typed || `${provider.name}-${realModel}`
}

function validate() {
  if (!form.value.providerId) return '请选择目标供应商'
  if (!form.value.upstreamProtocol) return '请选择上游协议'
  if (!conversionSupported.value) return conversionMessage.value
  if (!selectedGroup.value) return '当前供应商没有该上游协议对应的模型列表'
  if (!form.value.realModel) return '请选择真实模型'
  const provider = selectedProvider.value
  const clientModel = resolveClientModel(provider, form.value.realModel)
  const duplicated = props.protocol.mappings.some(
    (mapping) => mapping.clientModel === clientModel && mapping.id !== props.mapping?.id
  )
  if (duplicated) return `已有映射使用 ${clientModel}，同一个客户端模型只应保留一条`
  return ''
}

async function onConfirm() {
  error.value = validate()
  if (error.value) return

  const saved = await configStore.upsertMapping(props.protocol.id, {
    id: props.mapping?.id ?? newId('m'),
    enabled: props.mapping?.enabled ?? true,
    clientModel: resolveClientModel(selectedProvider.value, form.value.realModel),
    providerId: form.value.providerId,
    modelGroupId: selectedGroup.value.id,
    upstreamProtocol: form.value.upstreamProtocol,
    realModel: form.value.realModel
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
    :title="isEdit ? '编辑映射' : '添加映射'"
    width="480px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <FormField label="客户端模型" hint="支持 * 通配（如 claude-sonnet-4-*），单独一个 * 表示兜底，优先级最低；留空则自动用 供应商名-真实模型">
      <BaseInput v-model="form.clientModel" placeholder="留空自动用 供应商名-模型名" />
    </FormField>

    <FormField label="目标供应商" required>
      <AppSelect v-model="form.providerId" :options="providerOptions" placeholder="选择供应商" full-width />
    </FormField>

    <FormField label="上游协议" required hint="决定实际发往上游的接口格式；与客户端协议不同时走转换模块">
      <AppSelect v-model="form.upstreamProtocol" :options="upstreamOptions" placeholder="选择上游协议" full-width />
    </FormField>

    <p v-if="conversionMessage" class="conversion-note" :class="{ 'conversion-error': !conversionSupported }">
      {{ conversionMessage }}
    </p>

    <FormField label="真实模型" required>
      <AppSelect v-model="form.realModel" :options="modelOptions" placeholder="选择模型" full-width />
    </FormField>

    <p v-if="error" class="dialog-error">{{ error }}</p>

    <template #footer>
      <BaseButton variant="secondary" size="sm" @click="emit('update:modelValue', false)">取消</BaseButton>
      <BaseButton size="sm" :disabled="!conversionSupported" :loading="configStore.saving" @click="onConfirm">保存</BaseButton>
    </template>
  </BaseDialog>
</template>

<style scoped>
.dialog-error {
  margin: var(--spacing-md) 0 0;
  color: var(--color-error);
  font-size: var(--font-size-sm);
}

.conversion-note {
  margin: calc(var(--spacing-sm) * -1) 0 var(--spacing-md);
  color: var(--text-muted);
  font-size: var(--font-size-xs);
}

.conversion-error {
  color: var(--color-error);
}
</style>
