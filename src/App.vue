<script setup>
// 应用壳：顶部工具条 + 内容区。
// 顶栏只保留页面入口与代理状态；协议切换放在模型映射页的映射模块标题行。
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { useProxyStore } from '@/stores/useProxyStore'
import { useClientDirsStore } from '@/stores/useClientDirsStore'
import { Toggle } from '@/components/base'
import { logger } from '@/utils/logger'
import ProvidersView from '@/views/providers/ProvidersView.vue'
import LogsView from '@/views/logs/LogsView.vue'
import UsageView from '@/views/usage/UsageView.vue'
import SettingsView from '@/views/settings/SettingsView.vue'
import ProjectsView from '@/views/projects/ProjectsView.vue'

// 页面入口。模型映射页内部再切换客户端协议。
const VIEW_ITEMS = [
  { key: 'providers', label: '模型映射' },
  { key: 'logs', label: '请求日志' },
  { key: 'usage', label: '用量统计' },
  { key: 'settings', label: '设置' }
]

const configStore = useConfigStore()
const proxyStore = useProxyStore()
const clientDirsStore = useClientDirsStore()
const windowParams = new URLSearchParams(window.location.search)
const isProjectModelsWindow = windowParams.get('window') === 'project-models'
const projectProtocol = windowParams.get('protocol') || 'anthropic'
const projectMappingId = windowParams.get('mapping') || ''

const activeView = ref('providers')
const activeProtocolId = ref(null)

// 协议来自配置：配置被重建或协议列表变化后选中项可能失效，兜底回第一个，
// 免得两个模块拿到一个不存在的协议而渲染成空白
watch(
  () => configStore.protocols,
  (list) => {
    if (!list.some((item) => item.id === activeProtocolId.value)) activeProtocolId.value = list[0]?.id ?? null
  },
  { immediate: true }
)

async function onToggleProxy() {
  if (proxyStore.isRunning) await proxyStore.stop()
  else await proxyStore.start()
}

let unsubscribes = []

onMounted(async () => {
  if (isProjectModelsWindow) {
    return
  }
  unsubscribes.push(window.electronEvents.onConfigChanged((payload) => configStore.applyState(payload)))
  unsubscribes.push(window.electronEvents.onProxyStatusChanged((payload) => proxyStore.apply(payload)))
  await Promise.all([configStore.init(), proxyStore.init()])
  // 启动就扫一遍客户端目录：有和 CCProxy 不一致的，顶栏常驻一个感叹号提醒，
  // 点它去设置页处理。配置/代理先就位，检查的口径（端口、token）才是最终的
  clientDirsStore.init()
  // 界面起来、IPC 通了的证据落一条日志：窗口白屏或命令没注册时，翻 app.log 就知道卡在哪一步
  logger.info('[UI] 界面已就绪', {
    protocols: configStore.protocols.map((protocol) => protocol.id),
    proxy: proxyStore.status,
    configPath: configStore.configPath
  })
})

// 协议配置（映射、模型能力）一变，目录检查的口径就过期了——codex 的模型清单
// 对照的是 OpenAI Responses 协议的映射。重扫一遍，顶栏徽标跟着更新
watch(() => configStore.protocols, () => { clientDirsStore.reload() }, { deep: true })

onUnmounted(() => {
  unsubscribes.forEach((off) => off())
  unsubscribes = []
})
</script>

<template>
  <div v-if="isProjectModelsWindow" class="app-subwindow">
    <ProjectsView :protocol-id="projectProtocol" :mapping-id="projectMappingId" />
  </div>

  <div v-else class="app-shell">
    <header class="app-topbar" data-tauri-drag-region="deep">
      <span class="app-title">CCProxy</span>

      <div class="app-topbar-right">
        <!-- 客户端目录有和 CCProxy 不一致的：常驻一个感叹号，点它去设置页处理。
             跟横幅不同，它不是一件事的播报而是「有一处要管」的状态，修好自动消失，
             所以做成安静的圆点挂在切换控件前面，不抢横幅的戏 -->
        <button
          v-if="clientDirsStore.problemCount"
          type="button"
          class="app-dir-alert"
          :title="`${clientDirsStore.problemCount} 个客户端配置目录与 CCProxy 不一致，点击去处理`"
          @click="activeView = 'settings'"
        >
          !
        </button>

        <nav class="app-view-nav">
          <button
            v-for="item in VIEW_ITEMS"
            :key="item.key"
            type="button"
            class="app-view-item"
            :class="{ active: activeView === item.key }"
            @click="activeView = item.key"
          >
            {{ item.label }}
          </button>
        </nav>

        <div class="app-proxy">
          <span class="app-proxy-port">{{ proxyStore.isRunning ? `:${proxyStore.port}` : '未监听' }}</span>
          <!-- 启停用开关：开关本身就是「当前是否在跑」，比按钮文案随状态变更好读。
               操作中禁用，避免连点；状态以主进程回推为准，所以这里只绑显示值、不做 v-model -->
          <Toggle
            :model-value="proxyStore.isRunning"
            :disabled="proxyStore.busy"
            size="sm"
            :title="proxyStore.isRunning ? '停止代理' : '启动代理'"
            @change="onToggleProxy"
          />
        </div>
      </div>
    </header>

    <main class="app-main">
      <ProvidersView v-if="activeView === 'providers'" v-model:protocol-id="activeProtocolId" />
      <LogsView v-else-if="activeView === 'logs'" />
      <UsageView v-else-if="activeView === 'usage'" />
      <SettingsView v-else-if="activeView === 'settings'" />
    </main>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.app-subwindow {
  height: 100%;
  min-height: 0;
  overflow: hidden;
}

/* 全宽顶部工具条：整条可拖拽，红绿灯嵌在左侧（trafficLightPosition 18 正好落在 48px 高度的中线）。
   拖拽在 WKWebView 里靠 data-tauri-drag-region 属性，写在 template 上、值给 "deep"：
   整条子树都能拖，里面的按钮/链接等可点元素会自动挡掉拖拽，正好对应原来
   「顶栏 drag + 右侧 no-drag」那套写法。这条属性还要配 core:window:allow-start-dragging
   权限（见 src-tauri/capabilities/default.json），少了它按下去没反应；
   -webkit-app-region 是 Electron 时代的写法，留着只为标注意图，不再生效 */
.app-topbar {
  flex: 0 0 var(--header-height);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-lg);
  /* 右内边距与下面内容区的页面内边距取同一档，右边线才对得齐 */
  padding: 0 var(--spacing-xl) 0 92px;
  -webkit-app-region: drag;
  border-bottom: 1px solid var(--border-default);
  /* 顶栏不铺自己的底色，直接露出画布，靠下面这条线分隔（CC Switch 的 glass-header 就这么做）。
     之前用 --bg-secondary 铺底，它比画布还暗，顶栏看着像凹进去的槽 */
  background: transparent;
}

.app-title {
  font-size: var(--font-size-md);
  font-weight: var(--font-weight-semibold);
  color: var(--text-primary);
}

/* 客户端目录的告警圆点：小小一枚常驻提醒，只靠颜色和位置说话，不带文字 */
.app-dir-alert {
  flex: 0 0 auto;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 1px solid var(--color-warning);
  background: var(--color-warning-bg);
  color: var(--color-warning);
  font-size: var(--font-size-xs);
  font-weight: var(--font-weight-semibold);
  line-height: 1;
  cursor: pointer;
  padding: 0;
}

.app-dir-alert:hover {
  border-color: var(--color-warning-hover);
  color: var(--color-warning-hover);
}

/* 顶栏右侧整体不可拖拽：里面的按钮、Tab、下拉都要能接收点击 */
.app-topbar-right {
  display: flex;
  align-items: center;
  gap: var(--spacing-lg);
  -webkit-app-region: no-drag;
}

/* 三个页面入口做成一组分段控件（和协议 Tab 一样的容器写法），一眼能看出是一组 */
.app-view-nav {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 2px;
  background-color: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

.app-view-item {
  padding: var(--spacing-xs) 10px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-sm);
  font-weight: var(--font-weight-medium);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.app-view-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

/* 选中态不用橙色：橙色只留协议 Tab 一处，否则同一行两个橙色互相抢焦点 */
.app-view-item.active {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.app-proxy {
  display: flex;
  align-items: center;
  gap: var(--spacing-sm);
}

.app-proxy-port {
  color: var(--text-muted);
  font-size: var(--font-size-xs);
  font-family: var(--font-family-mono);
}

.app-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

</style>
