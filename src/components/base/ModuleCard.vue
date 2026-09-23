<script setup>
/**
 * ModuleCard - 模块外壳（CCProxy 自己加的，不在 CCGUI 那套基础组件里）
 *
 * 供应商主页上「映射模块」和「代理商模块」长得一模一样：面板色卡片 + 标题与说明 + 右上角主操作 +
 * 分隔线下的内容区。骨架放这里，两块各自只写自己的内容（表格 / 行列表），
 * 否则这 60 多行样式要在两个文件里各抄一份，改一处忘另一处。
 */
defineProps({
  title: {
    type: String,
    required: true
  },
  // 模块级提示（切换/删除失败的原因等），非空时显示在内容区顶上
  notice: {
    type: String,
    default: ''
  }
})
</script>

<template>
  <section class="module">
    <!-- 头部在卡片外：标题、说明与主操作落在页面底色上，卡片只包内容区 -->
    <header class="module-header">
      <div>
        <h3 class="module-title">{{ title }}</h3>
        <p class="module-desc"><slot name="desc" /></p>
      </div>
      <slot name="action" />
    </header>

    <div class="module-card">
      <div class="module-body">
        <p v-if="notice" class="module-notice">{{ notice }}</p>
        <slot />
      </div>
    </div>
  </section>
</template>

<style scoped>
/* 模块 = 卡片外的头部 + 卡片本体；两者间距比模块之间的间距（页面 gap）小，
   标题才能明确归属于下面这张卡片 */
.module {
  display: flex;
  flex-direction: column;
  gap: var(--spacing-sm);
}

/* 卡片只包内容区：面板色 + 描边 + 圆角 */
.module-card {
  background-color: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  overflow: hidden;
}

/* 头部在卡片外，与卡片左右对齐，不加水平内边距 */
.module-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--spacing-md);
}

.module-title {
  margin: 0 0 var(--spacing-xs);
  font-size: var(--font-size-md);
  font-weight: var(--font-weight-semibold);
  color: var(--text-primary);
}

.module-desc {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}

.module-desc code {
  padding: 1px 4px;
  background: var(--code-inline-bg);
  border-radius: var(--radius-sm);
  font-family: var(--font-family-mono);
}

.module-body {
  padding: var(--spacing-lg);
}

.module-notice {
  margin: 0 0 var(--spacing-md);
  padding: var(--spacing-sm) var(--spacing-md);
  background: var(--color-warning-bg);
  color: var(--color-warning);
  border-radius: var(--radius-md);
  font-size: var(--font-size-sm);
}

/* 空态在卡片里要收一档：模块高度是内容撑的，按整页那套留白（上下 32px）会撑得
   比一屏数据还高，于是「空的时候两块一样高、加了数据反而缩水」。收紧到约等于一行数据，
   加不加内容卡片高度都不跳 */
.module-body :deep(.empty-state) {
  padding: var(--spacing-lg) var(--spacing-md);
}

.module-body :deep(.empty-icon) {
  margin-bottom: var(--spacing-sm);
}

.module-body :deep(.empty-description) {
  margin-bottom: 0;
}

/* 配色另说：默认那套半透明占位风格在面板色卡片上太淡，提亮一档；
   标题也要比描述更亮，不然两条灰字分不出主次 */
.module-body :deep(.empty-icon) {
  color: var(--text-secondary);
  opacity: 0.8;
}

.module-body :deep(.empty-title) {
  color: var(--text-primary);
}

.module-body :deep(.empty-description) {
  color: var(--text-secondary);
}
</style>
