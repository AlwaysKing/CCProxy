<script setup>
// 统一项目管理：一个工作目录一行，直接维护 Claude 与 Codex 的项目默认模型。
import { computed, onMounted, onUnmounted } from 'vue'
import { useProjectModelsStore } from '@/stores/useProjectModelsStore'
import { AppSelect, Badge, BaseButton, BaseInput, EmptyState } from '@/components/base'
import ProjectStatusIcon from './components/ProjectStatusIcon.vue'

const store = useProjectModelsStore()
let unsubscribe = null

const sourceLabels = computed(() => (project) => {
  const labels = []
  if (project.sources?.claude) labels.push('Claude')
  if (project.sources?.codex) labels.push('Codex')
  return labels
})

function mappingIdFor(project, client) {
  return project[`${client}MappingId`] || ''
}

function modelFor(project, client) {
  return project[`${client}Model`] || ''
}

function statusFor(project, client) {
  return project[`${client}Status`]
}

function optionsFor(project, client) {
  const candidates = store.candidates[client] || []
  const options = [
    { value: '', label: '未设置', menuLabel: '不设置（停止托管）' },
    ...candidates.map((candidate) => ({
      value: candidate.mappingId,
      label: candidate.model
    }))
  ]
  const mappingId = mappingIdFor(project, client)
  if (mappingId && !candidates.some((candidate) => candidate.mappingId === mappingId)) {
    options.push({
      value: mappingId,
      label: `${modelFor(project, client) || mappingId}（当前值，不可重新选择）`,
      disabled: true
    })
  }
  return options
}

function canEdit(project, client) {
  return !project.missing || !!mappingIdFor(project, client)
}

function selectDisabled(project, client) {
  return !canEdit(project, client) || store.isPending(project.path, client)
}

function onModelChange(project, client, mappingId) {
  return store.setModel(project, client, mappingId)
}

onMounted(() => {
  store.reload()
  unsubscribe = window.electronEvents.onProjectModelsChanged(() => store.reload())
})

onUnmounted(() => {
  if (unsubscribe) unsubscribe()
})
</script>

<template>
  <div class="projects-page">
    <header class="page-bar">
      <div>
        <h1>项目管理</h1>
        <p class="page-subtitle">
          直接设置每个项目的 Claude 与 Codex 默认模型；目录按完整路径排序。
        </p>
      </div>
      <BaseButton
        variant="secondary"
        size="sm"
        :loading="store.loading"
        @click="store.reload(true)"
      >
        刷新项目
      </BaseButton>
    </header>

    <p v-if="store.notice" class="page-notice">{{ store.notice }}</p>

    <div class="page-card">
      <div class="table-toolbar">
        <BaseInput v-model="store.search" size="sm" placeholder="搜索项目路径" />
        <span>{{ store.filteredProjects.length }} 个项目</span>
      </div>

      <table v-if="store.filteredProjects.length" class="data-table">
        <thead>
          <tr>
            <th>项目</th>
            <th>Claude 默认模型</th>
            <th>Codex 默认模型</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="project in store.filteredProjects"
            :key="project.id"
            :class="{ 'row-missing': project.missing }"
          >
            <td class="project-cell">
              <code :title="project.path">{{ project.path }}</code>
              <div class="project-meta">
                <Badge
                  v-for="label in sourceLabels(project)"
                  :key="label"
                  variant="default"
                  size="sm"
                >
                  {{ label }}
                </Badge>
                <span v-if="project.missing" class="project-warning">目录暂不可用</span>
              </div>
            </td>
            <td>
              <div class="model-cell" :class="{ 'is-unset': !mappingIdFor(project, 'claude') }">
                <AppSelect
                  :model-value="mappingIdFor(project, 'claude')"
                  :options="optionsFor(project, 'claude')"
                  :disabled="selectDisabled(project, 'claude')"
                  menu-min-width="360px"
                  @update:model-value="onModelChange(project, 'claude', $event)"
                />
                <ProjectStatusIcon
                  v-if="statusFor(project, 'claude')"
                  client="claude"
                  :status="statusFor(project, 'claude')"
                />
              </div>
            </td>
            <td>
              <div class="model-cell" :class="{ 'is-unset': !mappingIdFor(project, 'codex') }">
                <AppSelect
                  :model-value="mappingIdFor(project, 'codex')"
                  :options="optionsFor(project, 'codex')"
                  :disabled="selectDisabled(project, 'codex')"
                  menu-min-width="360px"
                  @update:model-value="onModelChange(project, 'codex', $event)"
                />
                <ProjectStatusIcon
                  v-if="statusFor(project, 'codex')"
                  client="codex"
                  :status="statusFor(project, 'codex')"
                />
              </div>
            </td>
          </tr>
        </tbody>
      </table>

      <EmptyState
        v-else
        title="还没有发现项目"
        description="从 Codex 或 Claude 使用过的目录会自动出现在这里"
      />
    </div>

    <div class="page-card notes-card">
      <div>
        <strong>停止托管</strong>
        <span>
          “不设置”只删除 CCProxy 的管理关系，不会删除或回滚项目里已经写入的
          <code>model</code>。
        </span>
      </div>
      <div>
        <strong>生效方式</strong>
        <span>项目配置写入后，已运行的客户端通常要到新会话才读取新的默认模型。</span>
      </div>
      <div>
        <strong>Codex 项目信任</strong>
        <span>Codex 只会加载受信任项目的 <code>.codex/config.toml</code>。</span>
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
  </div>
</template>

<style scoped>
.projects-page {
  flex: 1;
  min-height: 0;
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
  width: min(360px, 45vw);
}

.data-table {
  table-layout: fixed;
}

.data-table th:nth-child(2),
.data-table th:nth-child(3) {
  width: 25%;
}

.project-cell {
  width: 50%;
  max-width: 0;
}

.project-cell > code {
  display: block;
  overflow: hidden;
  color: var(--text-primary);
  font-family: var(--font-family-mono);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-meta {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--spacing-xs);
  min-height: 18px;
  margin-top: 4px;
}

.project-warning {
  color: var(--color-warning);
  font-size: var(--font-size-xs);
}

.model-cell {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  min-width: 0;
}

.model-cell :deep(.app-select) {
  flex: 0 1 250px;
  width: min(250px, 100%);
  min-width: 0;
}

.model-cell.is-unset :deep(.app-select-trigger) {
  border-style: dashed;
  background: transparent;
  color: var(--text-secondary);
}

.model-cell.is-unset :deep(.app-select-trigger:hover),
.model-cell.is-unset :deep(.app-select.open .app-select-trigger) {
  border-style: solid;
  background: var(--bg-hover);
  color: var(--text-primary);
}

.row-missing .project-cell > code {
  color: var(--text-muted);
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
</style>
