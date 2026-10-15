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

const form = ref({ clientModel: '', providerId: '', realModel: '' })
const error = ref('')

const isEdit = computed(() => !!props.mapping)
// 供应商来自全局池：这份映射表之外的协议也能引用同一个供应商
const providerOptions = computed(() =>
  configStore.providers.map((provider) => ({ value: provider.id, label: provider.name }))
)
const selectedProvider = computed(
  () => configStore.providers.find((provider) => provider.id === form.value.providerId) || null
)
// 供应商的模型是配置条目，这里只关心名字——真实模型名进映射，属性留在供应商那边
function modelNames(provider) {
  return (provider?.models ?? []).map((model) => model.name)
}

const modelOptions = computed(() => {
  const models = modelNames(selectedProvider.value)
  const options = models.map((model) => ({ value: model, label: model }))
  // 编辑回填的旧值可能已不在该供应商的模型列表里（后来改过列表）：保留成一项并标出来，
  // 不然下拉显示成空的，用户一保存这个值就没了
  const current = form.value.realModel
  if (current && !models.includes(current)) {
    options.unshift({ value: current, label: `${current}（不在该供应商模型列表中）` })
  }
  return options
})

watch(
  () => props.modelValue,
  (visible) => {
    if (!visible) return
    error.value = ''
    // 新增时供应商和模型都默认选第一项：只有一个供应商的常见场景下能少点两次
    const first = configStore.providers[0]
    form.value = props.mapping
      ? {
          clientModel: props.mapping.clientModel,
          providerId: props.mapping.providerId,
          realModel: props.mapping.realModel
        }
      : { clientModel: '', providerId: first?.id ?? '', realModel: modelNames(first)[0] ?? '' }
  }
)

// 换供应商后旧模型大概率不在新列表里：换成新家的第一个，免得把甲的模型名发给乙。
// 只在真正「换」的时候动（id 变了才算），编辑回填不算
watch(
  () => form.value.providerId,
  (id, prevId) => {
    if (!id || !prevId || id === prevId) return
    const models = modelNames(configStore.providers.find((provider) => provider.id === id))
    if (!models.includes(form.value.realModel)) form.value.realModel = models[0] ?? ''
  }
)

function resolveClientModel(provider, realModel) {
  const typed = form.value.clientModel.trim()
  return typed || `${provider.name}-${realModel}`
}

function validate() {
  if (!form.value.providerId) return '请选择目标供应商'
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

    <FormField label="真实模型" required>
      <AppSelect v-model="form.realModel" :options="modelOptions" placeholder="选择模型" full-width />
    </FormField>

    <p v-if="error" class="dialog-error">{{ error }}</p>

    <template #footer>
      <BaseButton variant="secondary" size="sm" @click="emit('update:modelValue', false)">取消</BaseButton>
      <BaseButton size="sm" :loading="configStore.saving" @click="onConfirm">保存</BaseButton>
    </template>
  </BaseDialog>
</template>

<style scoped>
.dialog-error {
  margin: var(--spacing-md) 0 0;
  color: var(--color-error);
  font-size: var(--font-size-sm);
}
</style>
