<script setup>
// 映射模块：当前协议的 clientModel → 供应商 / 真实模型 映射表。
// 精确 > 通配 > 兜底 * 的优先级由主进程 router 决定，这里只把规则摊开给用户看并允许增删改启停。
import { computed, ref } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { Badge, BaseButton, ConfirmDialog, EmptyState, IconButton, ModuleCard, Toggle } from '@/components/base'
import MappingDialog from './dialogs/MappingDialog.vue'

const props = defineProps({
  protocol: {
    type: Object,
    required: true
  }
})

const configStore = useConfigStore()

const dialogVisible = ref(false)
const editingMapping = ref(null)
const deletingMapping = ref(null)
const notice = ref('')

// 没有供应商时映射没有目标可指，先挡在入口。供应商是全局池，所有协议共用同一份
const canAdd = computed(() => configStore.providers.length > 0)

// 规则形态要一眼能分辨：表格里全是模型名，通配和兜底光看字符串容易漏。
// 兜底用主色（橙）：蓝色是这套配色里唯一的冷色，挂在表里跟整页不合，且 info 语义偏「提示」，
// 而兜底是「最后一道承接」的强调意味，橙色更贴
function ruleKind(clientModel) {
  if (clientModel === '*') return { label: '兜底', variant: 'primary' }
  if (clientModel.includes('*')) return { label: '通配', variant: 'warning' }
  return { label: '精确', variant: 'default' }
}

// 一行要同时用到规则的中文名和徽章变体，先配好再渲染，免得在 v-for 里调两遍 ruleKind
const rows = computed(() =>
  props.protocol.mappings.map((mapping) => ({ mapping, kind: ruleKind(mapping.clientModel) }))
)

function providerName(providerId) {
  return configStore.providers.find((item) => item.id === providerId)?.name || '（供应商已删除）'
}

function onAdd() {
  if (!canAdd.value) {
    notice.value = '先去下面的代理商模块添加一个供应商，映射才有目标可指'
    return
  }
  notice.value = ''
  editingMapping.value = null
  dialogVisible.value = true
}

function onEdit(mapping) {
  notice.value = ''
  editingMapping.value = mapping
  dialogVisible.value = true
}

async function onToggle(mapping) {
  notice.value = ''
  // 启停也是落盘操作，失败要和删除/添加一样把原因亮出来，不能静默
  const ok = await configStore.toggleMapping(props.protocol.id, mapping.id)
  notice.value = ok ? '' : configStore.error || '操作失败'
}

async function onConfirmDelete() {
  const ok = await configStore.removeMapping(props.protocol.id, deletingMapping.value.id)
  notice.value = ok ? '' : configStore.error || '删除失败'
  deletingMapping.value = null
}
</script>

<template>
  <ModuleCard title="映射模块" :notice="notice">
    <template #desc>
      把客户端发来的模型名换成本协议下某个供应商的真实模型；匹配顺序为精确 &gt; 通配 &gt; 兜底 <code>*</code>。
    </template>
    <!-- 添加按钮只挂在标题右侧这一处，空态里不再放第二个：同一个动作两个位置各有一个，
         而且空态比一行数据高，卡片高度会在「空 / 有内容」之间跳 -->
    <template #action>
      <BaseButton size="sm" @click="onAdd">+ 添加映射</BaseButton>
    </template>

    <table v-if="protocol.mappings.length" class="data-table">
      <thead>
        <tr>
          <th>映射模型</th>
          <th>供应商</th>
          <th>原始模型</th>
          <th class="col-toggle">启停</th>
          <th class="col-actions">操作</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="row in rows" :key="row.mapping.id" :class="{ 'row-off': !row.mapping.enabled }">
          <td>
            <div class="cell-model">
              <code>{{ row.mapping.clientModel }}</code>
              <Badge :variant="row.kind.variant" size="sm">{{ row.kind.label }}</Badge>
            </div>
          </td>
          <td>
            <span class="cell-provider">{{ providerName(row.mapping.providerId) }}</span>
          </td>
          <td>
            <code class="cell-real">{{ row.mapping.realModel }}</code>
          </td>
          <td class="col-toggle">
            <Toggle size="sm" :model-value="row.mapping.enabled" @change="onToggle(row.mapping)" />
          </td>
          <td class="col-actions">
            <IconButton size="sm" title="编辑" @click="onEdit(row.mapping)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 20h9" />
                <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
              </svg>
            </IconButton>
            <IconButton size="sm" danger title="删除" @click="deletingMapping = row.mapping">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
                <path d="M10 11v6M14 11v6" />
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
              </svg>
            </IconButton>
          </td>
        </tr>
      </tbody>
    </table>

    <EmptyState
      v-else
      title="还没有映射规则"
      description="添加一条映射，把客户端模型名指到供应商的真实模型；建议至少留一条 * 兜底规则，未命中的请求才不会报 404"
    >
      <template #icon>
        <!-- 默认图标是「圆圈感叹号」，看着像坏掉的占位符；这里换成「分支」——一个客户端模型名分出去指向不同供应商 -->
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <line x1="6" y1="3" x2="6" y2="15" />
          <circle cx="18" cy="6" r="3" />
          <circle cx="6" cy="18" r="3" />
          <path d="M18 9a9 9 0 0 1-9 9" />
        </svg>
      </template>
    </EmptyState>

    <MappingDialog
      v-model="dialogVisible"
      :protocol="protocol"
      :mapping="editingMapping"
    />

    <ConfirmDialog
      :model-value="!!deletingMapping"
      title="删除映射"
      :description="deletingMapping ? `删除规则 ${deletingMapping.clientModel} → ${deletingMapping.realModel}？删除后命中它的请求会走下一优先级。` : ''"
      confirm-text="删除"
      :loading="configStore.saving"
      @update:model-value="deletingMapping = null"
      @confirm="onConfirmDelete"
    />
  </ModuleCard>
</template>

<style scoped>
/* 表格骨架（内边距、分隔线、悬停）在 global.css 的 .data-table 里，这里只放本模块独有的格子。
   表格在卡片里，自己不再铺底色和圆角，否则两块灰会叠在一起 */
.row-off {
  opacity: 0.5;
}

.cell-model {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

/* 映射模型与原始模型是同一类东西（都是模型名），样式必须一致，合起来写一条，
   改一处不会漏另一处。原始模型拆列前跟在供应商名后面，靠灰色小字表示从属；
   自己占一列后不再是从属信息，跟着回到主文字色 */
.cell-model code,
.cell-real {
  font-family: var(--font-family-mono);
  color: var(--text-primary);
}

.cell-provider {
  color: var(--text-primary);
}

.col-toggle {
  width: 64px;
}

/* 操作列和其他列一样左对齐：表头文字与图标同起点，不在列内一左一右 */
.col-actions {
  width: 110px;
  white-space: nowrap;
}
</style>
