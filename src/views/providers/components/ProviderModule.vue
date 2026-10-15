<script setup>
// 代理商模块：全局供应商池（增删改 + 启停）。
// 供应商不再属于某个协议：供应商保留名称、启停、地址与 token，模型按上游格式分组，
// 三个协议的映射表按供应商 id + 模型组 id 引用这里。
// 行不是可点的：走哪个供应商完全由映射模块的规则决定，这里点一下不该改变路由。
import { computed, ref } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { BaseButton, ConfirmDialog, EmptyState, IconButton, ModuleCard, Toggle } from '@/components/base'
import ProviderDialog from './dialogs/ProviderDialog.vue'

const configStore = useConfigStore()

const providers = computed(() => configStore.providers)

// 模型列表固定为 Claude 与 OpenAI 两类
const FORMAT_LABELS = {
  anthropic: 'Claude',
  openai: 'OpenAI'
}

function formatLabel(apiFormat) {
  return FORMAT_LABELS[apiFormat] || apiFormat
}

function modelCount(provider) {
  return (provider.modelGroups ?? []).reduce((total, group) => total + (group.models?.length ?? 0), 0)
}

// 支持的模型族图标：Claude 与 OpenAI 各一枚。OpenAI 的 chat 与 responses 在上游是同一个接口、
// 拉下来是同一份模型列表，所以按族去重，一个供应商最多出两枚图标
const FAMILY_BY_FORMAT = {
  anthropic: 'anthropic',
  openai: 'openai'
}

function modelFamilies(provider) {
  const families = []
  for (const group of provider.modelGroups ?? []) {
    const family = FAMILY_BY_FORMAT[group.apiFormat]
    if (family && !families.includes(family)) families.push(family)
  }
  return families
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
      上游供应商共用地址与 token，模型列表只分 Claude / OpenAI 两类；三个协议共用同一份供应商池
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
          <div class="provider-head">
            <span class="provider-name">{{ provider.name }}</span>
            <!-- 支持的模型族：用品牌图标代替文字徽章，同一族只出一枚。
                 图标本身没有文字，全称挂在 title 上，鼠标停上去还能看全 -->
            <span class="provider-formats">
              <span
                v-for="family in modelFamilies(provider)"
                :key="family"
                :title="formatLabel(family)"
              >
                <svg v-if="family === 'anthropic'" width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                  <path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" />
                </svg>
                <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                  <path d="M9.20509 8.76514V6.50548C9.20509 6.31517 9.27649 6.17237 9.44293 6.07733L13.9861 3.46091C14.6046 3.10416 15.342 2.93772 16.103 2.93772C18.9573 2.93772 20.7651 5.14986 20.7651 7.50457C20.7651 7.67101 20.7651 7.86132 20.7412 8.05164L16.0316 5.29243C15.7462 5.12599 15.4607 5.12599 15.1753 5.29243L9.20509 8.76514ZM19.8136 17.566V12.1665C19.8136 11.8334 19.6708 11.5955 19.3854 11.4291L13.4152 7.95637L15.3656 6.83836C15.5321 6.74332 15.6749 6.74332 15.8413 6.83836L20.3845 9.45477C21.6928 10.216 22.5728 11.8334 22.5728 13.4032C22.5728 15.2108 21.5025 16.8759 19.8136 17.5657V17.566ZM7.80173 12.8088L5.8513 11.6671C5.68486 11.5721 5.61346 11.4293 5.61346 11.239V6.00616C5.61346 3.46114 7.56389 1.53436 10.2042 1.53436C11.2033 1.53436 12.1307 1.86746 12.9159 2.46205L8.2301 5.17374C7.94475 5.34018 7.80196 5.57801 7.80196 5.91112V12.809L7.80173 12.8088ZM12 15.2349L9.20509 13.6651V10.3352L12 8.76537L14.7947 10.3352V13.6651L12 15.2349ZM13.7958 22.4659C12.7967 22.4659 11.8693 22.1328 11.0841 21.5382L15.7699 18.8265C16.0553 18.6601 16.198 18.4223 16.198 18.0892V11.1912L18.1723 12.3329C18.3388 12.4279 18.4102 12.5707 18.4102 12.7611V17.9939C18.4102 20.5389 16.4359 22.4657 13.7958 22.4657V22.4659ZM8.15848 17.1617L3.61528 14.5453C2.30696 13.784 1.42701 12.1667 1.42701 10.5969C1.42701 8.76537 2.52115 7.12417 4.20987 6.43431V11.8575C4.20987 12.1906 4.35266 12.4284 4.63802 12.5948L10.5846 16.0437L8.63415 17.1617C8.46771 17.2567 8.32492 17.2567 8.15848 17.1617ZM7.897 21.0626C5.20919 21.0626 3.23488 19.0407 3.23488 16.5432C3.23488 16.3529 3.25875 16.1626 3.2824 15.9723L7.96817 18.684C8.25352 18.8504 8.53911 18.8504 8.82447 18.684L14.7947 15.2351V17.4948C14.7947 17.6851 14.7233 17.8279 14.5568 17.9229L10.0136 20.5394C9.39518 20.8961 8.6578 21.0626 7.89677 21.0626H7.897ZM13.7958 23.8929C16.6739 23.8929 19.0762 21.8475 19.6235 19.1358C22.2874 18.4459 24 15.9484 24 13.4034C24 11.7383 23.2865 10.121 22.002 8.95546C22.121 8.45591 22.1924 7.95637 22.1924 7.45705C22.1924 4.05573 19.4331 1.51048 16.2458 1.51048C15.6037 1.51048 14.9852 1.60553 14.3668 1.81971C13.2963 0.773101 11.8215 0.107117 10.2042 0.107117C7.32606 0.107117 4.92383 2.15259 4.37653 4.86428C1.7126 5.55414 0 8.05164 0 10.5967C0 12.2617 0.713506 13.8791 1.99795 15.0446C1.87904 15.5441 1.80764 16.0437 1.80764 16.543C1.80764 19.9443 4.56685 22.4896 7.75421 22.4896C8.39632 22.4896 9.01478 22.3945 9.63324 22.1803C10.7035 23.2269 12.1783 23.8929 13.7958 23.8929Z" />
                </svg>
              </span>
            </span>
          </div>
          <span class="provider-url" :title="provider.baseUrl">{{ provider.baseUrl }}</span>
        </div>

        <div class="provider-models">
          <span v-if="modelCount(provider) === 0" class="provider-models-empty">未配置模型</span>
          <template v-else>
            <span v-for="group in provider.modelGroups.filter((item) => item.models.length).slice(0, 2)" :key="group.id" class="provider-model">
              {{ formatLabel(group.apiFormat) }} {{ group.models.length }}
            </span>
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
      description="先添加一个供应商：名称、上游地址与 token。下面按上游格式各自维护模型列表，并用自己的协议拉取"
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
/* 供应商列表：和映射表格一致，用行间分隔线区分，不再靠留白分块 */
.provider-list {
  display: flex;
  flex-direction: column;
}

.provider-row {
  display: flex;
  align-items: center;
  gap: var(--spacing-md);
  padding: var(--spacing-sm) var(--spacing-md);
  border-bottom: 1px solid var(--border-default);
}

.provider-row:last-child {
  border-bottom: none;
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
  /* 名称和图标挤在同一行了：名字过长时截断，别把后面的图标顶出去 */
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.provider-url {
  color: var(--text-secondary);
  font-family: var(--font-family-mono);
  font-size: var(--font-size-xs);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 名称与图标同一行：图标表示「这家上游支持哪些模型族」，贴着名称比单占一列更好读 */
.provider-head {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
  min-width: 0;
}

/* 品牌图标本身没有底色和描边，用次级字色，跟这套单色界面一致 */
.provider-formats {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
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
