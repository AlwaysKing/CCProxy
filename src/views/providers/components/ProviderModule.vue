<script setup>
// 代理商模块：全局供应商池（增删改 + 启停）。
// 供应商不再属于某个协议——「一个上游端点 + 一种上游格式」与客户端协议无关，
// 三个协议的映射表按 id 引用这里，同一家上游不用在每个协议下各录一遍。
// 行不是可点的：走哪个供应商完全由映射模块的规则决定，这里点一下不该改变路由。
import { computed, ref } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { BaseButton, ConfirmDialog, EmptyState, IconButton, ModuleCard, Toggle } from '@/components/base'
import ProviderDialog from './dialogs/ProviderDialog.vue'

const configStore = useConfigStore()

const providers = computed(() => configStore.providers)

// 上游格式的显示名：auto 是「不做转换」，其余三种要转换到对应格式
const FORMAT_LABELS = {
  auto: '跟随客户端',
  anthropic: 'Anthropic',
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses'
}

function formatLabel(apiFormat) {
  return FORMAT_LABELS[apiFormat] || FORMAT_LABELS.auto
}

// auto 不用高亮：它是不转换的那一类，标出来只是让人确认没选错
function isExplicitFormat(apiFormat) {
  return !!apiFormat && apiFormat !== 'auto'
}

const dialogVisible = ref(false)
const editingProvider = ref(null)
const deletingProvider = ref(null)
const notice = ref('')

function onAdd() {
  notice.value = ''
  editingProvider.value = null
  dialogVisible.value = true
}

function onEdit(provider) {
  notice.value = ''
  editingProvider.value = provider
  dialogVisible.value = true
}

async function onToggle(provider) {
  notice.value = ''
  // 启停也是落盘操作，失败要和切换/删除一样把原因亮出来，不能静默
  const ok = await configStore.toggleProvider(provider.id)
  notice.value = ok ? '' : configStore.error || '操作失败'
}

async function onConfirmDelete() {
  const ok = await configStore.removeProvider(deletingProvider.value.id)
  notice.value = ok ? '' : configStore.error || '删除失败'
  deletingProvider.value = null
}

// 删除供应商会连带删掉指向它的映射（store 里就是这么实现的）。
// 供应商是全局的，所以要跨三个协议数一遍，删之前把涉及哪些协议说清楚
const deletingUsages = computed(() =>
  deletingProvider.value ? configStore.mappingsUsingProvider(deletingProvider.value.id) : []
)

const deletingDescription = computed(() => {
  const provider = deletingProvider.value
  if (!provider) return ''
  const usages = deletingUsages.value
  if (usages.length === 0) return `删除供应商「${provider.name}」？`
  const protocols = [...new Set(usages.map((item) => item.protocolName))].join('、')
  return `删除供应商「${provider.name}」？${protocols} 下指向它的 ${usages.length} 条映射会一并删除。`
})
</script>

<template>
  <ModuleCard title="代理商模块" :notice="notice">
    <template #desc>
      上游供应商（地址 + token + 上游格式 + 模型列表），三个协议共用。
      同一家上游配一次即可，各协议的映射表按名称引用它
    </template>
    <!-- 添加按钮只挂在标题右侧这一处，空态里不再放第二个：同一个动作两个位置各有一个，
         而且空态比一行数据高，卡片高度会在「空 / 有内容」之间跳 -->
    <template #action>
      <BaseButton size="sm" @click="onAdd">+ 添加供应商</BaseButton>
    </template>

    <div v-if="providers.length" class="provider-list">
      <div
        v-for="provider in providers"
        :key="provider.id"
        class="provider-row"
        :class="{ 'provider-row-off': !provider.enabled }"
      >
        <!-- 首字方块：一列行扫下去时靠它分块，也省得给每个供应商配图标 -->
        <span class="provider-avatar">{{ provider.name.slice(0, 1) }}</span>

        <div class="provider-info">
          <span class="provider-name">{{ provider.name }}</span>
          <span class="provider-url" :title="provider.baseUrl">{{ provider.baseUrl }}</span>
        </div>

        <!-- 上游格式决定「要不要转换」：auto 是透传，其余三种会转换到该格式 -->
        <span class="provider-format" :class="{ 'provider-format-explicit': isExplicitFormat(provider.apiFormat) }">
          {{ formatLabel(provider.apiFormat) }}
        </span>

        <div class="provider-models">
          <span v-if="provider.models.length === 0" class="provider-models-empty">未配置模型</span>
          <template v-else>
            <span v-for="model in provider.models.slice(0, 2)" :key="model.name" class="provider-model">{{ model.name }}</span>
            <span v-if="provider.models.length > 2" class="provider-model">+{{ provider.models.length - 2 }}</span>
          </template>
        </div>

        <div class="provider-actions">
          <Toggle size="sm" :model-value="provider.enabled" title="启停" @change="onToggle(provider)" />
          <IconButton size="sm" title="编辑" @click="onEdit(provider)">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 20h9" />
              <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
            </svg>
          </IconButton>
          <IconButton size="sm" danger title="删除" @click="deletingProvider = provider">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="3 6 5 6 21 6" />
              <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
              <path d="M10 11v6M14 11v6" />
              <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
            </svg>
          </IconButton>
        </div>
      </div>
    </div>

    <EmptyState
      v-else
      title="还没有供应商"
      description="先添加一个供应商：名称、上游地址、token、上游格式与模型列表。配好后到映射模块把客户端模型指过来"
    >
      <template #icon>
        <!-- 默认图标是「圆圈感叹号」，看着像坏掉的占位符；这里换成「服务器」——这块管的就是上游机器 -->
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="2" width="20" height="8" rx="2" ry="2" />
          <rect x="2" y="14" width="20" height="8" rx="2" ry="2" />
          <line x1="6" y1="6" x2="6.01" y2="6" />
          <line x1="6" y1="18" x2="6.01" y2="18" />
        </svg>
      </template>
    </EmptyState>

    <ProviderDialog
      v-model="dialogVisible"
      :provider="editingProvider"
    />

    <ConfirmDialog
      :model-value="!!deletingProvider"
      title="删除供应商"
      :description="deletingDescription"
      confirm-text="删除"
      :loading="configStore.saving"
      @update:model-value="deletingProvider = null"
      @confirm="onConfirmDelete"
    />
  </ModuleCard>
</template>

<style scoped>
/* 供应商列表：一行一个供应商，行之间留 2px 靠留白分块，不用再套一层卡片 */
.provider-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.provider-row {
  display: flex;
  align-items: center;
  gap: var(--spacing-md);
  padding: var(--spacing-sm) var(--spacing-md);
  border: 1px solid transparent;
  border-radius: var(--radius-lg);
}

/* 停用的整行压暗：它不参与路由（router 会跳过），也不该被误认为正在服务 */
.provider-row-off {
  opacity: 0.55;
}

.provider-avatar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background-color: var(--bg-hover);
  border-radius: var(--radius-md);
  color: var(--text-primary);
  font-size: var(--font-size-base);
  font-weight: var(--font-weight-semibold);
}

.provider-info {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.provider-name {
  color: var(--text-primary);
  font-size: var(--font-size-md);
  font-weight: var(--font-weight-medium);
}

.provider-url {
  color: var(--text-secondary);
  font-family: var(--font-family-mono);
  font-size: var(--font-size-xs);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.provider-models {
  flex: 0 1 auto;
  display: flex;
  align-items: center;
  gap: var(--spacing-xs);
  font-size: var(--font-size-xs);
}

.provider-model {
  padding: 1px 6px;
  background: var(--bg-tertiary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  font-family: var(--font-family-mono);
}

.provider-models-empty {
  color: var(--color-warning);
}

/* 上游格式：auto（透传）不抢眼，显式指定了格式才用强调色——那是会走转换的那一类 */
.provider-format {
  flex: 0 0 auto;
  padding: 2px 8px;
  background: var(--bg-tertiary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  white-space: nowrap;
}

.provider-format-explicit {
  border-color: var(--color-primary);
  color: var(--color-primary);
}

.provider-actions {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}
</style>
