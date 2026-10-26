<script setup>
// 项目默认模型：一个工作目录一行，分别同步 Codex 与 Claude Code 的顶层 model。
import { computed, onMounted } from 'vue'
import { useProjectModelsStore } from '@/stores/useProjectModelsStore'
import { Badge, BaseButton, BaseInput, EmptyState, Toggle } from '@/components/base'
import AppSelect from '@/components/base/AppSelect.vue'

const store = useProjectModelsStore()

const STATUS_META = {
  ok: { label: '已同步', variant: 'success' },
  sync_disabled: { label: '同步已关闭', variant: 'default' },
  binding_invalid: { label: '绑定失效', variant: 'warning' },
  project_missing: { label: '目录不存在', variant: 'warning' },
  config_unreadable: { label: '配置不可读', variant: 'error' },
  write_failed: { label: '写入失败', variant: 'error' },
  restart_required: { label: '待重启 Codex', variant: 'warning' }
}

function withUnset(models) {
  return [{ value: '', label: '不设置' }, ...models.map((model) => ({ value: model, label: model }))]
}

const codexOptions = computed(() => withUnset(store.candidates.codex))
const claudeOptions = computed(() => withUnset(store.candidates.claude))

function sourceLabels(project) {
  const labels = []
  if (project.manual) labels.push('手动')
  if (project.sources?.codex) labels.push('Codex')
  if (project.sources?.claude) labels.push('Claude')
  if (!labels.length) labels.push('已保存')
  return labels
}

function statusFor(project, client) {
  return client === 'codex' ? project.codexStatus : project.claudeStatus
}

function statusMeta(project, client, model) {
  const status = statusFor(project, client)
  if (!status) {
    return model
      ? { label: '等待同步', variant: 'default' }
      : null
  }
  return STATUS_META[status.status] || { label: status.status, variant: 'default' }
}

function shortPath(path) {
  const home = '/Users/'
  if (!path.startsWith(home)) return path
  const parts = path.split('/')
  return parts.length > 4 ? `…/${parts.slice(-3).join('/')}` : path
}

function onModelChange(project, client, model) {
  return store.setModel(project, client, model)
}

onMounted(() => {
  store.reload()
})
</script>

<template>
  <div class="projects-page">
    <header class="page-bar">
      <div>
        <div class="page-title-line">
          <h2>项目默认模型</h2>
          <Badge :variant="store.enabled ? 'success' : 'default'" size="sm">
            {{ store.enabled ? '已启用' : '已关闭' }}
          </Badge>
        </div>
        <p class="page-subtitle">
          按工作目录固定 Codex 与 Claude Code 的默认模型；只写入项目配置里的 <code>model</code>。
        </p>
      </div>
      <Toggle
        :model-value="store.enabled"
        :disabled="store.busy"
        size="md"
        title="开启或关闭项目模型同步"
        @change="store.setEnabled"
      />
    </header>

    <p v-if="store.notice" class="page-notice">{{ store.notice }}</p>

    <div v-if="!store.enabled" class="page-card feature-off">
      <strong>项目模型同步已关闭</strong>
      <span>
        已有的项目绑定和客户端配置都会保留。开启后会重新发现项目，并对所有有效绑定执行一次同步。
      </span>
    </div>

    <div class="page-card toolbar-card">
      <div class="toolbar-search">
        <BaseInput
          v-model="store.search"
          size="sm"
          placeholder="搜索项目路径"
          :disabled="!store.enabled"
        />
      </div>
      <label class="hidden-filter">
        <Toggle
          :model-value="store.showHidden"
          size="sm"
          :disabled="!store.enabled"
          @change="store.showHidden = $event"
        />
        显示已隐藏
      </label>
      <span class="toolbar-count">
        {{ store.enabled ? `${store.filteredProjects.length} 个项目` : '' }}
      </span>
      <BaseButton
        variant="secondary"
        size="sm"
        :loading="store.busy"
        :disabled="!store.enabled"
        @click="store.refresh"
      >
        刷新项目
      </BaseButton>
      <BaseButton
        size="sm"
        :disabled="!store.enabled || store.busy"
        @click="store.add"
      >
        + 添加项目
      </BaseButton>
    </div>

    <div class="page-card project-list-card">
      <div v-if="store.loading && !store.projects.length" class="loading-state">正在读取项目…</div>

      <table v-else-if="store.filteredProjects.length" class="data-table project-table">
        <thead>
          <tr>
            <th>项目</th>
            <th>来源</th>
            <th>Codex 默认模型</th>
            <th>Claude Code 默认模型</th>
            <th class="col-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="project in store.filteredProjects"
            :key="project.id"
            :class="{ 'is-hidden': project.hidden, 'is-missing': project.missing }"
          >
            <td class="project-cell">
              <code class="project-path" :title="project.path">{{ shortPath(project.path) }}</code>
              <span v-if="project.missing" class="project-warning">目录暂不可用</span>
            </td>
            <td>
              <div class="source-list">
                <Badge
                  v-for="label in sourceLabels(project)"
                  :key="label"
                  variant="default"
                  size="sm"
                >
                  {{ label }}
                </Badge>
                <Badge v-if="project.hidden" variant="warning" size="sm">已隐藏</Badge>
              </div>
            </td>
            <td>
              <div class="model-cell">
                <AppSelect
                  :model-value="project.codexModel || ''"
                  :selected-label="project.codexModel || ''"
                  :options="codexOptions"
                  :disabled="store.busy || !store.enabled || project.missing"
                  full-width
                  @change="onModelChange(project, 'codex', $event.value)"
                />
                <div
                  v-if="statusMeta(project, 'codex', project.codexModel)"
                  class="model-status"
                  :title="statusFor(project, 'codex')?.message || ''"
                >
                  <Badge
                    :variant="statusMeta(project, 'codex', project.codexModel).variant"
                    size="sm"
                  >
                    {{ statusMeta(project, 'codex', project.codexModel).label }}
                  </Badge>
                </div>
              </div>
            </td>
            <td>
              <div class="model-cell">
                <AppSelect
                  :model-value="project.claudeModel || ''"
                  :selected-label="project.claudeModel || ''"
                  :options="claudeOptions"
                  :disabled="store.busy || !store.enabled || project.missing"
                  full-width
                  @change="onModelChange(project, 'claude', $event.value)"
                />
                <div
                  v-if="statusMeta(project, 'claude', project.claudeModel)"
                  class="model-status"
                  :title="statusFor(project, 'claude')?.message || ''"
                >
                  <Badge
                    :variant="statusMeta(project, 'claude', project.claudeModel).variant"
                    size="sm"
                  >
                    {{ statusMeta(project, 'claude', project.claudeModel).label }}
                  </Badge>
                </div>
              </div>
            </td>
            <td class="col-actions">
              <BaseButton
                variant="ghost"
                size="sm"
                :disabled="store.busy"
                @click="store.hide(project)"
              >
                {{ project.hidden ? '显示' : '隐藏' }}
              </BaseButton>
              <BaseButton
                variant="ghost"
                size="sm"
                :disabled="store.busy"
                @click="store.remove(project)"
              >
                移除
              </BaseButton>
            </td>
          </tr>
        </tbody>
      </table>

      <EmptyState
        v-else-if="store.enabled"
        title="还没有发现项目"
        description="从 Codex 或 Claude 使用过的目录会自动出现在这里，也可以手动添加"
      />
      <EmptyState
        v-else
        title="项目模型同步未开启"
        description="开启后才会扫描客户端已有的项目历史"
      />
    </div>

    <div v-if="store.enabled" class="page-card notes-card">
      <div>
        <strong>生效方式</strong>
        <span>已运行的客户端通常要到新会话才读取新的默认模型。</span>
      </div>
      <div v-if="store.daemon?.running" class="daemon-note">
        <strong>Codex app-server 正在运行</strong>
        <span>
          项目配置已经写入，但 daemon 只在启动时读取。执行
          <code>{{ store.daemon.restartCommand }}</code>
          后生效，正在运行的会话会被打断。
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

.page-title-line {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

.page-title-line h2 {
  font-size: var(--font-size-lg);
  font-weight: var(--font-weight-semibold);
}

.page-subtitle,
.feature-off span,
.notes-card span {
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}

.page-subtitle {
  margin-top: var(--spacing-xs);
}

.feature-off,
.notes-card > div {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-xs);
}

.toolbar-card {
  display: flex;
  align-items: center;
  gap: var(--spacing-md);
}

.toolbar-search {
  width: min(320px, 34vw);
}

.hidden-filter {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  white-space: nowrap;
}

.toolbar-count {
  margin-left: auto;
  color: var(--text-muted);
  font-size: var(--font-size-xs);
}

.project-list-card {
  padding-top: var(--spacing-md);
  padding-bottom: var(--spacing-md);
}

.project-table th,
.project-table td {
  padding-top: 10px;
  padding-bottom: 10px;
}

.project-table .col-actions {
  width: 128px;
  text-align: right;
}

.project-cell {
  max-width: 320px;
}

.project-path {
  display: block;
  overflow: hidden;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-warning {
  display: block;
  margin-top: 3px;
  color: var(--color-warning);
  font-size: var(--font-size-xs);
}

.source-list,
.model-cell {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

.source-list {
  flex-wrap: wrap;
}

.model-cell {
  min-width: 190px;
}

.model-cell :deep(.app-select) {
  min-width: 150px;
}

.model-status {
  flex: 0 0 auto;
}

.is-hidden {
  opacity: 0.62;
}

.is-missing .project-path {
  color: var(--text-muted);
}

.loading-state {
  padding: var(--spacing-xl);
  color: var(--text-muted);
  text-align: center;
}

.notes-card {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-md);
}

.daemon-note {
  padding-top: var(--spacing-md);
  border-top: 1px solid var(--border-default);
}
</style>
