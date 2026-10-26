<script setup>
// 协议隔离的项目默认模型子窗口：Anthropic 管 Claude，OpenAI Responses 管 Codex。
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useProjectModelsStore } from '@/stores/useProjectModelsStore'
import { Badge, BaseButton, BaseDialog, BaseInput, EmptyState, FormField } from '@/components/base'

const props = defineProps({
  protocolId: {
    type: String,
    required: true
  },
  mappingId: {
    type: String,
    required: true
  }
})

const store = useProjectModelsStore()
const addVisible = ref(false)
const selectedPaths = ref([])
const search = ref('')
const candidateSearch = ref('')
let unsubscribe = null

const STATUS_META = {
  ok: { label: '已同步', variant: 'success' },
  binding_invalid: { label: '绑定失效', variant: 'warning' },
  project_missing: { label: '目录不存在', variant: 'warning' },
  config_unreadable: { label: '配置不可读', variant: 'error' },
  write_failed: { label: '写入失败', variant: 'error' },
  restart_required: { label: '待重启 Codex', variant: 'warning' },
  requires_project_trust: { label: '需信任项目', variant: 'warning' }
}

const filteredProjects = computed(() => {
  const needle = search.value.trim().toLowerCase()
  return store.view.projects.filter((project) => !needle || project.path.toLowerCase().includes(needle))
})

const boundPaths = computed(() => new Set(store.view.projects.map((project) => project.path)))

const availableCandidates = computed(() => {
  const needle = candidateSearch.value.trim().toLowerCase()
  return store.view.directoryCandidates.filter(
    (path) => !boundPaths.value.has(path) && (!needle || path.toLowerCase().includes(needle))
  )
})

function statusMeta(project) {
  if (!project.status) return { label: '等待同步', variant: 'default' }
  return STATUS_META[project.status.status] || { label: project.status.status, variant: 'default' }
}

function toggleCandidate(path) {
  selectedPaths.value = selectedPaths.value.includes(path)
    ? selectedPaths.value.filter((item) => item !== path)
    : [...selectedPaths.value, path]
}

function openAdd() {
  selectedPaths.value = []
  candidateSearch.value = ''
  addVisible.value = true
}

async function confirmAdd() {
  if (!selectedPaths.value.length) return
  const ok = await store.add(props.protocolId, props.mappingId, selectedPaths.value)
  if (ok) addVisible.value = false
}

onMounted(() => {
  store.reload(props.protocolId, props.mappingId)
  unsubscribe = window.electronEvents.onProjectModelsChanged(() =>
    store.reload(props.protocolId, props.mappingId)
  )
})

onUnmounted(() => {
  if (unsubscribe) unsubscribe()
})
</script>

<template>
  <div class="projects-page">
    <header class="page-bar">
      <div>
        <h1>{{ store.view.title }}</h1>
        <p class="page-subtitle">
          从全局会话目录选择项目，固定未来会话使用的默认模型。移除绑定不会删除项目配置。
        </p>
      </div>
      <div class="bar-actions">
        <BaseButton
          variant="secondary"
          size="sm"
          :loading="store.loading"
          @click="store.reload(protocolId, mappingId, true)"
        >
          刷新候选
        </BaseButton>
        <BaseButton size="sm" :disabled="!availableCandidates.length" @click="openAdd">
          + 添加项目
        </BaseButton>
      </div>
    </header>

    <p v-if="store.notice" class="page-notice">{{ store.notice }}</p>

    <div class="page-card">
      <div class="table-toolbar">
        <BaseInput v-model="search" size="sm" placeholder="搜索项目路径" />
        <span>{{ filteredProjects.length }} 个项目</span>
      </div>

      <table v-if="filteredProjects.length" class="data-table">
        <thead>
          <tr>
            <th>项目</th>
            <th>默认模型</th>
            <th>状态</th>
            <th class="col-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="project in filteredProjects" :key="project.id">
            <td class="project-cell">
              <code :title="project.path">{{ project.path }}</code>
              <span v-if="project.missing" class="project-warning">目录暂不可用</span>
            </td>
            <td><code>{{ store.view.model }}</code></td>
            <td>
              <Badge :variant="statusMeta(project).variant" size="sm">
                {{ statusMeta(project).label }}
              </Badge>
              <span
                v-if="project.status?.message"
                class="status-message"
                :title="project.status.message"
              >{{ project.status.message }}</span>
            </td>
            <td class="col-actions">
              <BaseButton
                variant="ghost"
                size="sm"
                :disabled="store.busy"
                @click="store.remove(protocolId, mappingId, project.id)"
              >移除</BaseButton>
            </td>
          </tr>
        </tbody>
      </table>

      <EmptyState
        v-else
        title="还没有项目绑定"
        description="从全局会话目录中批量选择项目，为它们设置同一个默认模型"
      />
    </div>

    <div class="page-card notes-card">
      <div>
        <strong>生效方式</strong>
        <span>项目配置写入后，已运行的客户端通常要到新会话才读取新的默认模型。</span>
      </div>
      <div v-if="store.view.client === 'codex'">
        <strong>Codex 项目信任</strong>
        <span>只有受信任项目的 <code>.codex/config.toml</code> 才会被加载。</span>
      </div>
      <div v-if="store.view.codexDaemon?.running" class="daemon-note">
        <strong>Codex app-server 正在运行</strong>
        <span>
          配置已写入，但 daemon 只在启动时读取。执行
          <code>{{ store.view.codexDaemon.restartCommand }}</code>
          后生效。
        </span>
      </div>
    </div>

    <BaseDialog
      v-model="addVisible"
      title="添加项目"
      width="680px"
    >
      <FormField
        label="会话目录候选"
        :hint="`默认模型 ${store.view.model} · 已选 ${selectedPaths.length} 个目录`"
      >
        <BaseInput v-model="candidateSearch" size="sm" placeholder="搜索候选目录" />
        <div v-if="availableCandidates.length" class="candidate-list">
          <label
            v-for="path in availableCandidates"
            :key="path"
            class="candidate-row"
            :class="{ selected: selectedPaths.includes(path) }"
          >
            <input
              type="checkbox"
              :checked="selectedPaths.includes(path)"
              @change="toggleCandidate(path)"
            />
            <code>{{ path }}</code>
          </label>
        </div>
        <p v-else class="candidate-empty">当前协议没有新的会话目录候选</p>
      </FormField>

      <template #footer>
        <BaseButton variant="secondary" size="sm" @click="addVisible = false">取消</BaseButton>
        <BaseButton
          size="sm"
          :loading="store.busy"
          :disabled="!selectedPaths.length"
          @click="confirmAdd"
        >添加 {{ selectedPaths.length ? `(${selectedPaths.length})` : '' }}</BaseButton>
      </template>
    </BaseDialog>
  </div>
</template>

<style scoped>
.projects-page {
  height: 100%;
  padding: var(--spacing-xl);
  display: flex;
  flex-direction: column;
  gap: var(--spacing-lg);
  overflow-y: auto;
}

.page-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-lg);
}

.page-bar h1 {
  font-size: var(--font-size-lg);
  font-weight: var(--font-weight-semibold);
}

.page-subtitle,
.notes-card span {
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}

.page-subtitle {
  margin-top: var(--spacing-xs);
}

.bar-actions {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

.table-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-lg);
  margin-bottom: var(--spacing-md);
  color: var(--text-muted);
  font-size: var(--font-size-xs);
}

.table-toolbar :deep(.base-input-wrapper) {
  width: min(340px, 45vw);
}

.project-cell {
  max-width: 390px;
}

.project-cell code {
  display: block;
  overflow: hidden;
  font-family: var(--font-family-mono);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-warning {
  display: block;
  margin-top: 3px;
  color: var(--color-warning);
  font-size: var(--font-size-xs);
}

.status-message {
  display: block;
  max-width: 240px;
  overflow: hidden;
  margin-top: 3px;
  color: var(--text-muted);
  font-size: var(--font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.col-actions {
  width: 70px;
  text-align: right;
}

.notes-card {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-md);
}

.notes-card > div {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-xs);
}

.daemon-note {
  padding-top: var(--spacing-md);
  border-top: 1px solid var(--border-default);
}

.candidate-list {
  max-height: 280px;
  overflow-y: auto;
  margin-top: var(--spacing-sm);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

.candidate-row {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  padding: var(--spacing-sm) var(--spacing-md);
  cursor: pointer;
}

.candidate-row + .candidate-row {
  border-top: 1px solid var(--border-default);
}

.candidate-row:hover,
.candidate-row.selected {
  background-color: var(--bg-hover);
}

.candidate-row code {
  overflow: hidden;
  font-family: var(--font-family-mono);
  font-size: var(--font-size-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.candidate-empty {
  margin-top: var(--spacing-sm);
  color: var(--text-muted);
  font-size: var(--font-size-sm);
}
</style>
