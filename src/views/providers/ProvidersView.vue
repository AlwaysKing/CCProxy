<script setup>
// 供应商主页：按当前客户端协议分两块——映射模块与代理商模块。
// 协议切换位于映射模块标题行，本组件只负责把选中协议分发给映射模块。
import { computed } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { EmptyState } from '@/components/base'
import MappingModule from './components/MappingModule.vue'
import ProviderModule from './components/ProviderModule.vue'

const props = defineProps({
  protocolId: {
    type: String,
    default: null
  }
})
const emit = defineEmits(['update:protocolId'])

const configStore = useConfigStore()

const protocol = computed(() => configStore.protocols.find((item) => item.id === props.protocolId) || null)
const protocolTabs = computed(() => configStore.protocols.map((item) => ({ value: item.id, label: item.name })))
</script>

<template>
  <div class="providers-page">
    <!-- 切协议要换实例：模块自己攒着 notice（切换/删除失败的原因），沿用同一个实例会把上一个协议的
         报错显示在新协议上。两块必须各用自己的 key，同一组兄弟节点里 key 撞车会被 Vue 报警 -->
    <template v-if="protocol">
      <MappingModule
        :key="`mapping-${protocol.id}`"
        :protocol="protocol"
        :protocol-id="protocolId"
        :protocol-tabs="protocolTabs"
        @update:protocol-id="emit('update:protocolId', $event)"
      />
      <!-- 供应商是全局池，不随协议 Tab 切换，所以不带 key、也不传协议 -->
      <ProviderModule />
    </template>

    <EmptyState
      v-else
      title="没有可用的协议"
      description="配置里没有协议条目，检查 ~/.ccproxy/app.json 或重启应用让配置重建"
    >
      <template #icon>
        <!-- 和其他空态一样换掉默认的「圆圈感叹号」，用「分层」表示协议是配置里的若干条目 -->
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <polygon points="12 2 2 7 12 12 22 7 12 2" />
          <polyline points="2 17 12 22 22 17" />
          <polyline points="2 12 12 17 22 12" />
        </svg>
      </template>
    </EmptyState>
  </div>
</template>

<style scoped>
.providers-page {
  flex: 1;
  min-height: 0;
  /* 页面内边距和顶栏的右内边距同档，卡片右边线和顶栏右端对得齐；两块之间不用留太大，
     卡片自己有边框，留 16 就够分开 */
  padding: var(--spacing-xl);
  display: flex;
  flex-direction: column;
  gap: var(--spacing-lg);
  overflow-y: auto;
}

/* 页面级空态（配置里连协议条目都没有）跟模块里的空态用同一个亮度，
   否则又是默认那套半透明占位风格，在画布上淡得看不见 */
.providers-page :deep(.empty-icon) {
  color: var(--text-secondary);
  opacity: 0.8;
}

.providers-page :deep(.empty-title) {
  color: var(--text-primary);
}

.providers-page :deep(.empty-description) {
  color: var(--text-secondary);
}
</style>
