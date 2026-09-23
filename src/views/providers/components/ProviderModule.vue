<script setup>
// 代理商模块：当前协议下的供应商列表（增删改 + 启停）。
// 行不是可点的：走哪个供应商完全由上面映射模块的规则决定，这里点一下不该改变路由。
import { computed, ref } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { BaseButton, ConfirmDialog, EmptyState, IconButton, ModuleCard, Toggle } from '@/components/base'
import ProviderDialog from './dialogs/ProviderDialog.vue'

const props = defineProps({
  protocol: {
    type: Object,
    required: true
  }
})

const configStore = useConfigStore()

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
  const ok = await configStore.toggleProvider(props.protocol.id, provider.id)
  notice.value = ok ? '' : configStore.error || '操作失败'
}

async function onConfirmDelete() {
  const ok = await configStore.removeProvider(props.protocol.id, deletingProvider.value.id)
  notice.value = ok ? '' : configStore.error || '删除失败'
  deletingProvider.value = null
}

// 删除供应商会连带删掉指向它的映射（store 里就是这么实现的），删之前先把条数说清楚
const deletingMappingCount = computed(() =>
  deletingProvider.value
    ? props.protocol.mappings.filter((mapping) => mapping.providerId === deletingProvider.value.id).length
    : 0
)
</script>

<template>
  <ModuleCard title="代理商模块" :notice="notice">
    <template #desc>
      上游供应商（地址 + token + 模型列表）。兜底走哪一个，由上面映射模块的 <code>*</code> 规则决定
    </template>
    <!-- 添加按钮只挂在标题右侧这一处，空态里不再放第二个：同一个动作两个位置各有一个，
         而且空态比一行数据高，卡片高度会在「空 / 有内容」之间跳 -->
    <template #action>
      <BaseButton size="sm" @click="onAdd">+ 添加供应商</BaseButton>
    </template>

    <div v-if="protocol.providers.length" class="provider-list">
      <div
        v-for="provider in protocol.providers"
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

        <div class="provider-models">
          <span v-if="provider.models.length === 0" class="provider-models-empty">未配置模型</span>
          <template v-else>
            <span v-for="model in provider.models.slice(0, 2)" :key="model" class="provider-model">{{ model }}</span>
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
      description="先添加一个供应商：名称、上游地址、token 与模型列表。配好后到上面的映射模块把客户端模型指过来"
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
      :protocol="protocol"
      :provider="editingProvider"
    />

    <ConfirmDialog
      :model-value="!!deletingProvider"
      title="删除供应商"
      :description="deletingProvider
        ? `删除供应商「${deletingProvider.name}」？${deletingMappingCount > 0 ? `指向它的 ${deletingMappingCount} 条映射会一并删除。` : ''}`
        : ''"
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

.provider-actions {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}
</style>
