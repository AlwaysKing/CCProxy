// 渲染层构建配置：只打包 src/ 与 index.html。Rust 侧（src-tauri/）不参与打包，
// 它的 frontendDist 指向这里的 dist/
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

const rootDir = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [vue()],
  base: './',
  resolve: {
    alias: { '@': path.resolve(rootDir, 'src') }
  },
  server: {
    host: '127.0.0.1',
    port: 5184,
    strictPort: true
  },
  build: {
    outDir: 'dist'
  }
});
