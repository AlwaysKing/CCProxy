# CCProxy M2a（Electron 壳 + 配置管理）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给已完成的代理核心套上 Electron 壳：窗口 + 托盘（按协议分组切换供应商）+ preload IPC，配上 `~/.ccproxy/app.json` 的原子读写与容错，以及能把配置读进来、改回去、并能启停代理的 Vue 界面骨架。

**Architecture:** 主进程 `electron/main.cjs` 是唯一 IPC 边界：`ipcMain.handle` 全部集中注册（照 CCGUI 习惯），业务落在 `electron/storage/config-manager.js`（纯 Node，可单测）与 `electron/services/proxy/`（M1 已完成）。托盘拆成 `tray-menu.js`（纯函数，出菜单模板，可单测）+ `tray.js`（Electron 胶水）。渲染层照 CCGUI：Vue 3 `<script setup>` 纯 JS，Pinia setup-store 里直接调 `window.electronAPI`，无 vue-router（`computed` + `v-if` 切页），纯 CSS + `variables.css` 变量。

**Tech Stack:** Electron 26 + Vue 3.4 + Pinia 3 + Vite 5，纯 JS，主进程 CommonJS。

**里程碑拆分说明：** M2 规模较大，拆成两份计划。本计划 = **M2a 壳与配置**，交付「能跑起来、能读能写配置、能启停代理、托盘能切供应商」的可运行应用。**M2b 供应商界面**（协议 Tabs / 供应商卡片 / 映射表 / 编辑弹窗 / 设置页）在 M2a 执行完并验收后另写计划——M2b 的组件要依赖 M2a 实际搬进来的基础组件 API，先写会写偏。

**已完成的前置（M1）：** `electron/services/proxy/` 六个模块 + 5 个测试文件，`npm test` 35/35 通过。接口：`createLocalProxy({ getConfig, onLog, maxBodyBytes })` → `{ start() → {port}, stop(), getStatus() → {running, port} }`；`getConfig()` 每请求现读，改配置即时生效。

---

## 约定（全程适用）

- 所有代码放 `/Users/alwaysking/AKProject/CCProxy/`
- **不做任何 git 提交**（用户要求仅在明确指示时提交）
- 临时文件 / 调查脚本一律放 `/tmp/ccproxy/`，不写进项目
- **代码风格与 CCProxy 已有代码（M1）保持一致**：2 空格缩进、单引号、**写分号**、Node 文件带 `'use strict'`、中文注释、kebab-case 文件名、文件头写职责注释。
  - 注意：CCGUI 主进程**不写分号、不写 `'use strict'`**。这里选择与本项目已有代码统一，而不是照抄 CCGUI 的字面风格；照抄的是它的**结构习惯**（分层、IPC kebab-case 命名、`{ success, ... }` 返回形状、JSON Lines 日志、vite/electron-builder 配置形态、`variables.css` 变量体系、`components/base/` 组件约定）。
- Node 侧新代码尽量做成纯函数以便单测；Electron/Vue 胶水不写自动测试，靠 `npm run electron:dev` 手工验收
- 每个任务按 TDD：先写测试 → 跑一遍确认失败 → 写实现 → 跑通

## 三个已定的判断（写在这里备查）

1. **配置文件的 `dist/` 产物留在项目内**（`vite.config.mjs` 的 `build.outDir: 'dist'`，已在 `.gitignore`）。理由：`dist/` 是项目自身的构建产物、需要被 `loadFile` 读取，不属于规范里说的「临时/调查文件」。本阶段只跑 dev 模式，不执行 `vite build`。
2. **`activeProviderId` 改为派生字段**：以该协议的 `*` 兜底规则为准，`validateConfig` 在加载与保存时都重算它。理由：设计里「点卡片写 activeProviderId」和「`*` 规则决定实际路由」是两份状态，若各自可写就会打架；派生后只有一份真源，界面蓝边与托盘选中态都跟着 `*` 规则走。字段仍然写进 `app.json`（设计里的示例有它），只是不再作为输入。
3. **加单实例锁**（CCGUI 没有）。理由：代理占固定端口，且托盘应用多开会同时出现两个托盘图标并必然 `EADDRINUSE`，单实例是这类应用的刚需。

---

### Task 1: 依赖与构建配置

**Files:**
- Modify: `package.json`
- Create: `vite.config.mjs`
- Create: `index.html`
- Modify: `.gitignore`

- [ ] **Step 1: 改 `package.json`（整份替换）**

依赖版本照抄 CCGUI 的 `package.json`，只留本阶段需要的。

```json
{
  "name": "ccproxy",
  "version": "0.1.0",
  "private": true,
  "description": "本地 AI agent 协议代理：识别协议、映射模型、替换凭据、转发上游",
  "main": "electron/main.cjs",
  "engines": {
    "node": ">=21"
  },
  "scripts": {
    "test": "node --test 'tests/**/*.test.js'",
    "dev": "vite",
    "build": "vite build",
    "electron": "npx electron .",
    "electron:dev": "concurrently \"npm run dev\" \"wait-on http://127.0.0.1:5184 && npx electron .\" --kill-others",
    "electron:build": "npm run build && electron-builder"
  },
  "dependencies": {
    "pinia": "^3.0.4",
    "vue": "^3.4.0"
  },
  "devDependencies": {
    "@vitejs/plugin-vue": "^5.0.0",
    "concurrently": "^8.2.0",
    "electron": "^26.6.10",
    "electron-builder": "^24.9.0",
    "vite": "^5.0.0",
    "wait-on": "^7.2.0"
  },
  "build": {
    "appId": "com.ccproxy.app",
    "productName": "CCProxy",
    "directories": {
      "output": "output"
    },
    "mac": {
      "category": "public.app-category.developer-tools",
      "artifactName": "${productName}-${version}-${arch}.${ext}"
    }
  }
}
```

- [ ] **Step 2: 建 `vite.config.mjs`**

只配渲染层（主进程与 preload 不打包，作为裸 CJS 随包发布，与 CCGUI 一致）。dev 端口用 **5184**，避开 CCGUI 的 5183。

```js
// 渲染层构建配置：只打包 src/ 与 index.html；主进程与 preload 不参与打包
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
```

- [ ] **Step 3: 建 `index.html`（项目根目录）**

```html
<!DOCTYPE html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>CCProxy</title>
  </head>
  <body style="background: #111315">
    <div id="app"></div>
    <script type="module" src="/src/main.js"></script>
  </body>
</html>
```

- [ ] **Step 4: 改 `.gitignore`**

```
node_modules/
dist/
output/
.DS_Store
.vscode/
```

- [ ] **Step 5: 装依赖**

Run: `cd /Users/alwaysking/AKProject/CCProxy && npm install`
Expected: 安装完成无 ERESOLVE 报错（本机 `~/.npm/_cacache` 与 `~/Library/Caches/electron` 已有缓存，electron 二进制不必重新下载）。

- [ ] **Step 6: 校验依赖可用**

Run: `npx electron --version && npx vite --version`
Expected: 分别打印 `v26.x.x` 与 `v5.x.x`。

- [ ] **Step 7: 确认 M1 测试没被破坏**

Run: `npm test`
Expected: `# pass 35`，0 失败。

---

### Task 2: 配置管理 config-manager.js

配置读写全部逻辑集中在这一个文件：默认配置、校验归一化、原子写、损坏回落，以及「切换当前供应商」这条非平凡规则（托盘和界面都要用，放这里保证语义唯一）。

**Files:**
- Create: `electron/storage/config-manager.js`
- Test: `tests/unit/config-manager.test.js`

- [ ] **Step 1: 写失败测试**

```js
// 配置管理单测：默认配置、原子写、损坏回落、校验归一化、切换当前供应商
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const {
  createConfigManager,
  createDefaultConfig,
  validateConfig,
  setActiveProvider,
  DEFAULT_PORT
} = require('../../electron/storage/config-manager');

// 每个用例一个临时目录（写在 /tmp 下，不污染项目）
function makeManager(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-cfg-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return { dir, manager: createConfigManager({ dir }) };
}

function provider(id, models = ['glm-4.6']) {
  return { id, name: id, enabled: true, baseUrl: `https://${id}.example.com`, token: 'sk-x', models };
}
function mapping(id, clientModel, providerId, realModel = 'glm-4.6') {
  return { id, enabled: true, clientModel, providerId, realModel };
}

test('首次运行：文件不存在时返回默认配置且标记 firstRun', (t) => {
  const { manager } = makeManager(t);
  const result = manager.load();
  assert.equal(result.firstRun, true);
  assert.equal(result.recovered, false);
  assert.equal(result.config.settings.port, DEFAULT_PORT);
  assert.deepEqual(result.config.protocols.map((p) => p.id), ['anthropic', 'openai']);
});

test('保存后重新读取内容一致', (t) => {
  const { manager } = makeManager(t);
  const config = createDefaultConfig();
  config.protocols[0].providers.push(provider('p_zhipu'));
  config.protocols[0].mappings.push(mapping('m1', 'claude-*', 'p_zhipu'));
  manager.save(config);
  const reloaded = manager.load();
  assert.equal(reloaded.config.protocols[0].providers[0].id, 'p_zhipu');
  assert.equal(reloaded.config.protocols[0].mappings[0].clientModel, 'claude-*');
});

test('保存是原子的：不留 .tmp 残留且文件权限为 600', (t) => {
  const { dir, manager } = makeManager(t);
  manager.save(createDefaultConfig());
  assert.deepEqual(fs.readdirSync(dir), ['app.json']);
  assert.equal(fs.statSync(path.join(dir, 'app.json')).mode & 0o777, 0o600);
});

test('配置不是合法 JSON 时备份 .bak 并回落默认配置', (t) => {
  const { dir, manager } = makeManager(t);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'app.json'), '{ 这不是 json');
  const result = manager.load();
  assert.equal(result.recovered, true);
  assert.ok(result.backupPath.endsWith('app.json.bak'));
  assert.equal(fs.readFileSync(result.backupPath, 'utf8'), '{ 这不是 json');
  assert.equal(result.config.settings.port, DEFAULT_PORT);
});

test('配置根节点不是对象时判定为损坏', (t) => {
  const { dir, manager } = makeManager(t);
  fs.writeFileSync(path.join(dir, 'app.json'), '[1,2,3]');
  const result = manager.load();
  assert.equal(result.recovered, true);
  assert.equal(validateConfig([]).ok, false);
});

test('校验：端口非法回落默认并记 issue', () => {
  const result = validateConfig({ settings: { port: 'abc' }, protocols: [] });
  assert.equal(result.ok, true);
  assert.equal(result.config.settings.port, DEFAULT_PORT);
  assert.ok(result.issues.some((issue) => issue.includes('port')));
});

test('校验：未知协议 id 被丢弃并记 issue', () => {
  const result = validateConfig({
    settings: { port: 19000 },
    protocols: [
      { id: 'gemini', name: 'Gemini', enabled: true, providers: [], mappings: [] },
      { id: 'anthropic', name: 'Anthropic', enabled: true, providers: [], mappings: [] }
    ]
  });
  assert.deepEqual(result.config.protocols.map((p) => p.id), ['anthropic', 'openai']);
  assert.ok(result.issues.some((issue) => issue.includes('gemini')));
});

test('校验：缺 baseUrl 的供应商被丢弃，缺 providerId 的映射被丢弃', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [provider('ok'), { id: 'bad', name: 'bad', token: 't', models: [] }],
      mappings: [mapping('m1', 'a', 'ok'), { id: 'm2', clientModel: 'b', realModel: 'x' }]
    }]
  });
  const protocol = result.config.protocols[0];
  assert.deepEqual(protocol.providers.map((p) => p.id), ['ok']);
  assert.deepEqual(protocol.mappings.map((m) => m.id), ['m1']);
});

test('校验：映射指向不存在的供应商时被丢弃', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [provider('p1')], mappings: [mapping('m1', 'a', 'ghost')]
    }]
  });
  assert.equal(result.config.protocols[0].mappings.length, 0);
  assert.ok(result.issues.some((issue) => issue.includes('ghost')));
});

test('校验：activeProviderId 由 * 兜底规则派生', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      activeProviderId: 'p1',
      providers: [provider('p1'), provider('p2')],
      mappings: [mapping('m9', '*', 'p2')]
    }]
  });
  assert.equal(result.config.protocols[0].activeProviderId, 'p2');
});

test('校验：没有 * 兜底规则时 activeProviderId 为 null', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      activeProviderId: 'p1', providers: [provider('p1')], mappings: []
    }]
  });
  assert.equal(result.config.protocols[0].activeProviderId, null);
});

test('切换当前供应商：写入 activeProviderId 并创建 * 兜底规则', () => {
  const config = createDefaultConfig();
  config.protocols[0].providers.push(provider('p1', ['glm-4.6']));
  const next = setActiveProvider(config, 'anthropic', 'p1');
  const protocol = next.protocols[0];
  assert.equal(protocol.activeProviderId, 'p1');
  const fallback = protocol.mappings.find((m) => m.clientModel === '*');
  assert.equal(fallback.providerId, 'p1');
  assert.equal(fallback.realModel, 'glm-4.6');
  // 纯函数：原配置不被改动
  assert.equal(config.protocols[0].activeProviderId, null);
  assert.equal(config.protocols[0].mappings.length, 0);
});

test('切换当前供应商：已有 * 规则时只改指向、不新增', () => {
  const config = createDefaultConfig();
  config.protocols[0].providers.push(provider('p1'), provider('p2', ['glm-4.5-air']));
  config.protocols[0].mappings.push(mapping('m9', '*', 'p1', 'glm-4.6'));
  const next = setActiveProvider(config, 'anthropic', 'p2');
  assert.equal(next.protocols[0].mappings.length, 1);
  assert.equal(next.protocols[0].mappings[0].id, 'm9');
  assert.equal(next.protocols[0].mappings[0].providerId, 'p2');
  assert.equal(next.protocols[0].mappings[0].realModel, 'glm-4.5-air');
});

test('切换当前供应商：同一供应商重复点击不重置 realModel', () => {
  const config = createDefaultConfig();
  config.protocols[0].providers.push(provider('p1', ['glm-4.6']));
  config.protocols[0].mappings.push(mapping('m9', '*', 'p1', '手写的模型'));
  const next = setActiveProvider(config, 'anthropic', 'p1');
  assert.equal(next.protocols[0].mappings[0].realModel, '手写的模型');
});

test('切换当前供应商：供应商不存在或未配置模型时原样返回同一引用', () => {
  const config = createDefaultConfig();
  config.protocols[0].providers.push(provider('p1', []));
  assert.equal(setActiveProvider(config, 'anthropic', 'ghost'), config);
  assert.equal(setActiveProvider(config, 'anthropic', 'p1'), config);
  assert.equal(setActiveProvider(config, 'gemini', 'p1'), config);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `npm test 2>&1 | tail -20`
Expected: 报 `Cannot find module .../config-manager`。

- [ ] **Step 3: 写实现**

```js
// 配置读写：~/.ccproxy/app.json。原子写（tmp + rename）、损坏自动备份 .bak 后回落默认配置，
// 不因配置问题崩溃。校验与「切换当前供应商」都是纯函数，脱离 Electron 即可单测。

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

const CONFIG_VERSION = 1;
const DEFAULT_PORT = 18888;
const FALLBACK_MODEL = '*';
// v1 只支持这两个协议：识别器与适配器都按这个集合工作，配置里出现别的协议没有意义
const SUPPORTED_PROTOCOLS = [
  { id: 'anthropic', name: 'Anthropic' },
  { id: 'openai', name: 'OpenAI' }
];

function createDefaultConfig() {
  return {
    version: CONFIG_VERSION,
    settings: { port: DEFAULT_PORT, launchAtLogin: false },
    protocols: SUPPORTED_PROTOCOLS.map((preset) => ({
      id: preset.id,
      name: preset.name,
      enabled: true,
      activeProviderId: null,
      providers: [],
      mappings: []
    }))
  };
}

function createConfigId(prefix) {
  return `${prefix}_${crypto.randomUUID().replace(/-/g, '').slice(0, 8)}`;
}

function isPlainObject(value) {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function normalizeProvider(raw, protocolId, issues) {
  if (!isPlainObject(raw)) {
    issues.push(`协议 ${protocolId} 下存在非法供应商条目，已丢弃`);
    return null;
  }
  if (typeof raw.id !== 'string' || !raw.id) {
    issues.push(`协议 ${protocolId} 下有供应商缺少 id，已丢弃`);
    return null;
  }
  const baseUrl = typeof raw.baseUrl === 'string' ? raw.baseUrl.trim() : '';
  if (!baseUrl) {
    issues.push(`协议 ${protocolId} 的供应商 ${raw.id} 缺少 baseUrl，已丢弃`);
    return null;
  }
  const models = (Array.isArray(raw.models) ? raw.models : [])
    .filter((model) => typeof model === 'string' && model.trim())
    .map((model) => model.trim());
  return {
    id: raw.id,
    name: typeof raw.name === 'string' && raw.name.trim() ? raw.name.trim() : raw.id,
    enabled: raw.enabled !== false,
    baseUrl,
    token: typeof raw.token === 'string' ? raw.token : '',
    models
  };
}

function normalizeMapping(raw, protocolId, providerIds, issues) {
  if (!isPlainObject(raw)) {
    issues.push(`协议 ${protocolId} 下存在非法映射条目，已丢弃`);
    return null;
  }
  const fields = ['id', 'clientModel', 'providerId', 'realModel'];
  const missing = fields.filter((field) => typeof raw[field] !== 'string' || !raw[field].trim());
  if (missing.length > 0) {
    issues.push(`协议 ${protocolId} 下有映射缺少 ${missing.join('/')}，已丢弃`);
    return null;
  }
  if (!providerIds.has(raw.providerId)) {
    issues.push(`协议 ${protocolId} 的映射 ${raw.clientModel} 指向不存在的供应商 ${raw.providerId}，已丢弃`);
    return null;
  }
  return {
    id: raw.id,
    enabled: raw.enabled !== false,
    clientModel: raw.clientModel.trim(),
    providerId: raw.providerId,
    realModel: raw.realModel.trim()
  };
}

function buildProtocol(raw, preset, issues) {
  const protocol = {
    id: preset.id,
    name: preset.name,
    enabled: true,
    activeProviderId: null,
    providers: [],
    mappings: []
  };
  if (!isPlainObject(raw)) {
    issues.push(`协议 ${preset.id} 在配置中缺失，已用默认值补全`);
    return protocol;
  }
  protocol.enabled = raw.enabled !== false;
  if (typeof raw.name === 'string' && raw.name.trim()) protocol.name = raw.name.trim();

  const providerIds = new Set();
  for (const item of Array.isArray(raw.providers) ? raw.providers : []) {
    const provider = normalizeProvider(item, preset.id, issues);
    if (!provider) continue;
    if (providerIds.has(provider.id)) {
      issues.push(`协议 ${preset.id} 存在重复的供应商 id ${provider.id}，已丢弃后者`);
      continue;
    }
    providerIds.add(provider.id);
    protocol.providers.push(provider);
  }

  const mappingIds = new Set();
  for (const item of Array.isArray(raw.mappings) ? raw.mappings : []) {
    const mapping = normalizeMapping(item, preset.id, providerIds, issues);
    if (!mapping) continue;
    if (mappingIds.has(mapping.id)) {
      issues.push(`协议 ${preset.id} 存在重复的映射 id ${mapping.id}，已丢弃后者`);
      continue;
    }
    mappingIds.add(mapping.id);
    protocol.mappings.push(mapping);
  }

  // activeProviderId 是派生的：以 * 兜底规则为准，避免「当前启用」与实际路由两份状态打架
  const fallback = protocol.mappings.find((item) => item.clientModel === FALLBACK_MODEL);
  protocol.activeProviderId = fallback ? fallback.providerId : null;
  return protocol;
}

// 校验并归一化：能修的直接修，修不了的条目丢掉并记 issue。
// 只有「根节点不是对象」才判定为损坏（由调用方备份并回落默认配置）。
function validateConfig(raw) {
  if (!isPlainObject(raw)) {
    return { ok: false, config: null, issues: ['配置根节点不是对象'] };
  }
  const issues = [];
  const settings = isPlainObject(raw.settings) ? raw.settings : {};

  const port = Number(settings.port);
  const portValid = Number.isInteger(port) && port >= 1 && port <= 65535;
  if (!portValid) {
    issues.push(`settings.port 非法（${JSON.stringify(settings.port)}），已回落 ${DEFAULT_PORT}`);
  }

  const rawProtocols = Array.isArray(raw.protocols) ? raw.protocols : [];
  if (!Array.isArray(raw.protocols)) issues.push('protocols 不是数组，已重置为默认协议');

  const config = {
    version: CONFIG_VERSION,
    settings: {
      port: portValid ? port : DEFAULT_PORT,
      launchAtLogin: settings.launchAtLogin === true
    },
    protocols: SUPPORTED_PROTOCOLS.map((preset) => {
      const matched = rawProtocols.find((item) => isPlainObject(item) && item.id === preset.id);
      return buildProtocol(matched, preset, issues);
    })
  };

  for (const item of rawProtocols) {
    const id = isPlainObject(item) ? item.id : item;
    if (!SUPPORTED_PROTOCOLS.some((preset) => preset.id === id)) {
      issues.push(`丢弃不支持的协议：${JSON.stringify(id)}`);
    }
  }

  return { ok: true, config, issues };
}

// 切换「当前启用」供应商：写 activeProviderId，并把该协议的兜底规则 * 指向它（没有则创建）。
// 托盘菜单切换与界面点卡片都走这里，保证两处语义完全一致。
// 供应商不存在、或没配模型（生成不出真实模型名）时原样返回同一引用，调用方据此判断是否生效。
function setActiveProvider(config, protocolId, providerId) {
  const protocol = config.protocols.find((item) => item.id === protocolId);
  if (!protocol) return config;
  const provider = protocol.providers.find((item) => item.id === providerId);
  if (!provider || provider.models.length === 0) return config;

  const protocols = config.protocols.map((item) => {
    if (item.id !== protocolId) return item;
    const hasFallback = item.mappings.some((mapping) => mapping.clientModel === FALLBACK_MODEL);
    const mappings = hasFallback
      ? item.mappings.map((mapping) => {
          if (mapping.clientModel !== FALLBACK_MODEL) return mapping;
          // 换供应商时把真实模型名一并换成新供应商的第一个模型；同一供应商重复点击保留手写值
          const realModel = mapping.providerId === providerId ? mapping.realModel : provider.models[0];
          return { ...mapping, enabled: true, providerId, realModel };
        })
      : [
          ...item.mappings,
          {
            id: createConfigId('m'),
            enabled: true,
            clientModel: FALLBACK_MODEL,
            providerId,
            realModel: provider.models[0]
          }
        ];
    return { ...item, activeProviderId: providerId, mappings };
  });

  return { ...config, protocols };
}

function createConfigManager({ dir } = {}) {
  const baseDir = dir || path.join(os.homedir(), '.ccproxy');
  const configPath = path.join(baseDir, 'app.json');
  const tempPath = `${configPath}.tmp`;

  // 损坏配置留一份 .bak 再回落默认值，便于用户手工找回
  function backupBrokenConfig() {
    try {
      if (!fs.existsSync(configPath)) return null;
      const backupPath = `${configPath}.bak`;
      fs.renameSync(configPath, backupPath);
      return backupPath;
    } catch (error) {
      return null;
    }
  }

  function load() {
    let text;
    try {
      text = fs.readFileSync(configPath, 'utf8');
    } catch (error) {
      if (error.code === 'ENOENT') {
        return { config: createDefaultConfig(), issues: [], recovered: false, backupPath: null, firstRun: true };
      }
      const backupPath = backupBrokenConfig();
      return { config: createDefaultConfig(), issues: [error.message], recovered: true, backupPath, firstRun: false };
    }

    let raw;
    try {
      raw = JSON.parse(text);
    } catch (error) {
      const backupPath = backupBrokenConfig();
      return {
        config: createDefaultConfig(),
        issues: [`配置不是合法 JSON：${error.message}`],
        recovered: true,
        backupPath,
        firstRun: false
      };
    }

    const result = validateConfig(raw);
    if (!result.ok) {
      const backupPath = backupBrokenConfig();
      return { config: createDefaultConfig(), issues: result.issues, recovered: true, backupPath, firstRun: false };
    }
    return { config: result.config, issues: result.issues, recovered: false, backupPath: null, firstRun: false };
  }

  // 原子写：先写 .tmp 再 rename，避免写一半掉电留下半个文件
  function save(nextConfig) {
    const result = validateConfig(nextConfig);
    if (!result.ok) throw new Error(`配置校验失败：${result.issues.join('；')}`);

    fs.mkdirSync(baseDir, { recursive: true, mode: 0o700 });
    fs.writeFileSync(tempPath, `${JSON.stringify(result.config, null, 2)}\n`, { mode: 0o600 });
    fs.renameSync(tempPath, configPath);
    return { config: result.config, issues: result.issues };
  }

  return {
    load,
    save,
    getPath: () => configPath,
    getDir: () => baseDir
  };
}

module.exports = {
  createConfigManager,
  createDefaultConfig,
  createConfigId,
  validateConfig,
  setActiveProvider,
  DEFAULT_PORT,
  FALLBACK_MODEL,
  CONFIG_VERSION
};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `npm test 2>&1 | tail -20`
Expected: `# pass 50`（35 + 15），0 失败。

- [ ] **Step 5: 自查**

确认：`load()` 的四种返回都有 `config/issues/recovered/backupPath/firstRun` 五个字段；`setActiveProvider` 在失败路径返回的是**同一个引用**（调用方用 `===` 判断）；文件里没有 `console.log` 残留。

---

### Task 3: 主进程日志 logger.js

**Files:**
- Create: `electron/logger.js`
- Test: `tests/unit/logger.test.js`

- [ ] **Step 1: 写失败测试**

```js
// 日志单测：JSON Lines 格式、目录可切换、未初始化也不抛错
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const logger = require('../../electron/logger');

function makeDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-log-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test('初始化后写入的一行是合法 JSON 且含 level/message/data', (t) => {
  const dir = makeDir(t);
  logger.configure({ baseDir: dir, silent: true });
  logger.initialize();
  logger.info('[Proxy] 测试', { port: 18888 });

  const lines = fs.readFileSync(logger.getPath(), 'utf8').trim().split('\n');
  assert.equal(lines.length, 1);
  const entry = JSON.parse(lines[0]);
  assert.equal(entry.level, 'info');
  assert.equal(entry.message, '[Proxy] 测试');
  assert.deepEqual(entry.data, { port: 18888 });
  assert.ok(Date.parse(entry.timestamp) > 0);
});

test('未初始化也能写，目录自动创建且不抛错', (t) => {
  const dir = path.join(makeDir(t), 'nested');
  logger.configure({ baseDir: dir, silent: true });
  assert.doesNotThrow(() => logger.warn('未初始化'));
  assert.ok(fs.existsSync(logger.getPath()));
});

test('configure 切目录后写到新目录', (t) => {
  const first = makeDir(t);
  const second = makeDir(t);
  logger.configure({ baseDir: first, silent: true });
  logger.error('第一条');
  logger.configure({ baseDir: second, silent: true });
  logger.error('第二条');
  assert.match(fs.readFileSync(path.join(first, 'app.log'), 'utf8'), /第一条/);
  assert.match(fs.readFileSync(path.join(second, 'app.log'), 'utf8'), /第二条/);
});

test('data 为空时不写 data 字段', (t) => {
  const dir = makeDir(t);
  logger.configure({ baseDir: dir, silent: true });
  logger.error('无数据');
  const entry = JSON.parse(fs.readFileSync(logger.getPath(), 'utf8').trim().split('\n')[0]);
  assert.equal('data' in entry, false);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `npm test 2>&1 | tail -20`
Expected: 报 `Cannot find module .../logger`。

- [ ] **Step 3: 写实现**

日志落 `/tmp/ccproxy/app.log`（设计文档指定的位置，与 CCGUI 落 `/tmp/ccgui/` 同构）。格式 JSON Lines，一条一行。

```js
// 主进程日志：JSON Lines 写 /tmp/ccproxy/app.log，格式与 CCGUI 的 electron/logger.js 同构。
// 日志是旁路：写失败绝不能影响代理主流程。

'use strict';

const fs = require('fs');
const path = require('path');

const DEFAULT_BASE_DIR = '/tmp/ccproxy';
const LOG_FILE_NAME = 'app.log';
const LEVELS = { DEBUG: 'debug', INFO: 'info', WARN: 'warn', ERROR: 'error' };

let baseDir = DEFAULT_BASE_DIR;
let logPath = path.join(baseDir, LOG_FILE_NAME);
let silent = false;

// 切换日志目录（测试用）；silent 时不打控制台
function configure(options = {}) {
  if (options.baseDir) {
    baseDir = options.baseDir;
    logPath = path.join(baseDir, LOG_FILE_NAME);
  }
  if (typeof options.silent === 'boolean') silent = options.silent;
  return { baseDir, logPath };
}

function initialize() {
  try {
    fs.mkdirSync(baseDir, { recursive: true });
    return true;
  } catch (error) {
    return false;
  }
}

function log(level, message, data) {
  const entry = { timestamp: new Date().toISOString(), level, message };
  if (data !== undefined) entry.data = data;
  try {
    fs.mkdirSync(baseDir, { recursive: true });
    fs.appendFileSync(logPath, `${JSON.stringify(entry)}\n`);
  } catch (error) {
    // 日志写不进去也不能影响主流程
  }
  if (!silent) {
    const suffix = data === undefined ? '' : ` ${JSON.stringify(data)}`;
    console.log(`[${entry.timestamp}] ${level.toUpperCase()} ${message}${suffix}`);
  }
  return entry;
}

module.exports = {
  DEBUG: LEVELS.DEBUG,
  INFO: LEVELS.INFO,
  WARN: LEVELS.WARN,
  ERROR: LEVELS.ERROR,
  configure,
  initialize,
  debug: (message, data) => log(LEVELS.DEBUG, message, data),
  info: (message, data) => log(LEVELS.INFO, message, data),
  warn: (message, data) => log(LEVELS.WARN, message, data),
  error: (message, data) => log(LEVELS.ERROR, message, data),
  getPath: () => logPath
};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `npm test 2>&1 | tail -20`
Expected: `# pass 54`，0 失败。

---

### Task 4: 托盘图标与托盘菜单

托盘在 CCGUI 里没有先例（它只有 Dock 菜单），菜单模板抽成纯函数以便单测。图标用脚本生成：纯黑 + alpha 的 macOS 模板图（系统自动反色适配深浅色），运行中是实心圆 ●、停止是空心圆 ○。

**Files:**
- Create: `scripts/generate-tray-icons.js`
- Create: `build/icons/trayRunningTemplate.png`、`trayRunningTemplate@2x.png`、`trayStoppedTemplate.png`、`trayStoppedTemplate@2x.png`（由脚本生成）
- Create: `electron/tray-menu.js`
- Create: `electron/tray.js`
- Test: `tests/unit/tray-menu.test.js`

- [ ] **Step 1: 写失败测试**

```js
// 托盘菜单模板单测：状态行文案、按协议分组的供应商切换、不可用项的禁用
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { buildTrayMenu } = require('../../electron/tray-menu');

const baseActions = {
  switchProvider() {},
  openWindow() {},
  toggleProxy() {},
  quit() {}
};

function state(proxy, protocols) {
  return { proxy, config: { protocols } };
}
function protocol(id, name, activeProviderId, providers) {
  return { id, name, enabled: true, activeProviderId, providers, mappings: [] };
}
function provider(id, name, models = ['glm-4.6'], enabled = true) {
  return { id, name, enabled, baseUrl: 'https://x.example.com', token: 't', models };
}

test('运行中：状态行含端口，启停项为停止代理', () => {
  const menu = buildTrayMenu(state({ status: 'running', port: 18888, error: null }, []), baseActions);
  assert.match(menu[0].label, /18888/);
  assert.equal(menu[0].enabled, false);
  assert.ok(menu.some((item) => item.label === '停止代理'));
});

test('启动失败：状态行含错误信息，启停项为启动代理', () => {
  const menu = buildTrayMenu(state({ status: 'error', port: 18888, error: 'EADDRINUSE' }, []), baseActions);
  assert.match(menu[0].label, /EADDRINUSE/);
  assert.ok(menu.some((item) => item.label === '启动代理'));
});

test('按协议分组：供应商为 radio 且当前启用的选中', () => {
  const menu = buildTrayMenu(state({ status: 'stopped', port: 18888, error: null }, [
    protocol('anthropic', 'Anthropic', 'p2', [provider('p1', '甲'), provider('p2', '乙')]),
    protocol('openai', 'OpenAI', null, [])
  ]), baseActions);

  const anthropic = menu.find((item) => item.label === 'Anthropic');
  assert.equal(anthropic.submenu.length, 2);
  assert.equal(anthropic.submenu[0].type, 'radio');
  assert.equal(anthropic.submenu[0].checked, false);
  assert.equal(anthropic.submenu[1].checked, true);

  const openai = menu.find((item) => item.label === 'OpenAI');
  assert.equal(openai.submenu.length, 1);
  assert.equal(openai.submenu[0].enabled, false);
});

test('停用或未配置模型的供应商不可点', () => {
  const menu = buildTrayMenu(state({ status: 'stopped', port: 18888, error: null }, [
    protocol('anthropic', 'Anthropic', null, [provider('p1', '甲', ['glm-4.6'], false), provider('p2', '乙', [])])
  ]), baseActions);
  const anthropic = menu.find((item) => item.label === 'Anthropic');
  assert.equal(anthropic.submenu[0].enabled, false);
  assert.equal(anthropic.submenu[1].enabled, false);
});

test('点击供应商菜单项回调带协议 id 与供应商 id', () => {
  const calls = [];
  const menu = buildTrayMenu(
    state({ status: 'stopped', port: 18888, error: null }, [protocol('anthropic', 'Anthropic', null, [provider('p1', '甲')])]),
    { ...baseActions, switchProvider: (protocolId, providerId) => calls.push([protocolId, providerId]) }
  );
  menu.find((item) => item.label === 'Anthropic').submenu[0].click();
  assert.deepEqual(calls, [['anthropic', 'p1']]);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `npm test 2>&1 | tail -20`
Expected: 报 `Cannot find module .../tray-menu`。

- [ ] **Step 3: 写 `electron/tray-menu.js`**

```js
// 托盘菜单模板：纯函数，不 require electron，便于单测。
// 结构：状态行 → 按协议分组的供应商切换（radio）→ 打开主窗口 / 启停代理 → 退出。

'use strict';

const STATUS_TEXT = {
  running: '运行中',
  stopped: '已停止',
  error: '启动失败'
};

function buildStatusLabel(proxy) {
  if (proxy.status === 'running') return `● ${STATUS_TEXT.running} · 127.0.0.1:${proxy.port}`;
  const reason = proxy.error ? `：${proxy.error}` : '';
  return `○ ${STATUS_TEXT[proxy.status] || STATUS_TEXT.stopped}${reason}`;
}

function buildTrayMenu(state, actions) {
  const { proxy, config } = state;
  const template = [{ label: buildStatusLabel(proxy), enabled: false }, { type: 'separator' }];

  for (const protocol of config.protocols) {
    const submenu = protocol.providers.length === 0
      ? [{ label: '（未配置供应商）', enabled: false }]
      : protocol.providers.map((provider) => ({
          label: provider.enabled ? provider.name : `${provider.name}（已停用）`,
          type: 'radio',
          checked: provider.id === protocol.activeProviderId,
          // 没配模型的供应商切过去也生成不出兜底规则，直接不可点
          enabled: provider.enabled && provider.models.length > 0,
          click: () => actions.switchProvider(protocol.id, provider.id)
        }));
    template.push({ label: protocol.name, submenu });
  }

  template.push(
    { type: 'separator' },
    { label: '打开主窗口', click: () => actions.openWindow() },
    { label: proxy.status === 'running' ? '停止代理' : '启动代理', click: () => actions.toggleProxy() },
    { type: 'separator' },
    { label: '退出 CCProxy', click: () => actions.quit() }
  );

  return template;
}

module.exports = { buildTrayMenu };
```

- [ ] **Step 4: 跑测试确认通过**

Run: `npm test 2>&1 | tail -20`
Expected: `# pass 63`，0 失败。

- [ ] **Step 5: 写图标生成脚本**

`scripts/generate-tray-icons.js`：用 zlib 手写 PNG，零依赖。形状已试制确认（16 与 32 两档，实心圆 ● / 空心圆 ○）。

```js
// 生成 macOS 托盘模板图标（纯黑 + alpha，系统自动反色适配深浅色）。
// 输出 build/icons/tray{Running,Stopped}Template.png 与 @2x。手写 PNG，零依赖。
// 用法：node scripts/generate-tray-icons.js

'use strict';

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

function crc32(buf) {
  let c;
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = (crc ^ buf[i]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = c ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

// 8 位 RGBA（色彩类型 6），每行前置一个 0 滤波器字节
function makePng(size, alphaAt) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc(size * (size * 4 + 1));
  let cursor = 0;
  for (let y = 0; y < size; y++) {
    raw[cursor++] = 0;
    for (let x = 0; x < size; x++) {
      raw[cursor++] = 0;
      raw[cursor++] = 0;
      raw[cursor++] = 0;
      raw[cursor++] = alphaAt(x + 0.5, y + 0.5);
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

// 圆形边缘 1px 抗锯齿
function circleAlpha(distance, radius) {
  if (distance <= radius - 0.5) return 255;
  if (distance >= radius + 0.5) return 0;
  return Math.round(255 * (radius + 0.5 - distance));
}

function alphaFor(distance, spec) {
  if (spec.innerRadius > 0 && distance < spec.innerRadius) return 0;
  return circleAlpha(distance, spec.radius);
}

const SPECS = [
  { name: 'trayRunningTemplate.png', size: 16, radius: 6.2, innerRadius: 0 },
  { name: 'trayRunningTemplate@2x.png', size: 32, radius: 12.4, innerRadius: 0 },
  { name: 'trayStoppedTemplate.png', size: 16, radius: 6.2, innerRadius: 3.6 },
  { name: 'trayStoppedTemplate@2x.png', size: 32, radius: 12.4, innerRadius: 7.2 }
];

const outDir = path.join(__dirname, '..', 'build', 'icons');
fs.mkdirSync(outDir, { recursive: true });

for (const spec of SPECS) {
  const center = spec.size / 2;
  const png = makePng(spec.size, (x, y) => alphaFor(Math.hypot(x - center, y - center), spec));
  fs.writeFileSync(path.join(outDir, spec.name), png);
  console.log(`${spec.name}  ${png.length} bytes`);
}
```

- [ ] **Step 6: 生成图标并校验**

Run: `node scripts/generate-tray-icons.js && ls -l build/icons/`
Expected: 4 个文件，每个 140~230 bytes。

- [ ] **Step 7: 写 `electron/tray.js`**

```js
// 托盘：图标反映代理运行状态，菜单按协议分组切换当前供应商。
// 配置或状态变化后由 main.cjs 调 refresh() 重建菜单。

'use strict';

const path = require('path');
const { Tray, Menu, nativeImage } = require('electron');
const { buildTrayMenu } = require('./tray-menu');

const ICON_DIR = path.join(__dirname, '..', 'build', 'icons');

let tray = null;
let getState = null;
let actions = null;

// macOS 模板图：黑色 + alpha，系统按深浅色自动反色。@2x 由系统按同名规则自动加载
function iconFor(status) {
  const fileName = status === 'running' ? 'trayRunningTemplate.png' : 'trayStoppedTemplate.png';
  const image = nativeImage.createFromPath(path.join(ICON_DIR, fileName));
  image.setTemplateImage(true);
  return image;
}

function createTray(options) {
  getState = options.getState;
  actions = options.actions;
  tray = new Tray(iconFor(getState().proxy.status));
  tray.setToolTip('CCProxy');
  refresh();
  return tray;
}

function refresh() {
  if (!tray || tray.isDestroyed()) return;
  const state = getState();
  tray.setImage(iconFor(state.proxy.status));
  tray.setContextMenu(Menu.buildFromTemplate(buildTrayMenu(state, actions)));
}

function destroy() {
  if (tray && !tray.isDestroyed()) tray.destroy();
  tray = null;
}

module.exports = { createTray, refresh, destroy };
```

---

### Task 5: 主进程 main.cjs 与 preload.js

主进程只做三件事：窗口、托盘、IPC 边界。IPC channel 全部 kebab-case、全部在 `main.cjs` 集中注册（照 CCGUI），handler 统一 `try/catch` 返回 `{ success, ... }`。

**Files:**
- Create: `electron/main.cjs`
- Create: `electron/preload.js`

- [ ] **Step 1: 写 `electron/preload.js`**

```js
// preload：contextBridge 暴露 electronAPI（请求/响应）与 electronEvents（主进程推送订阅）。
// 订阅统一返回取消订阅函数。

'use strict';

const { contextBridge, ipcRenderer } = require('electron');

// 订阅统一返回取消订阅函数，组件卸载时直接调
function subscribe(channel, callback) {
  const listener = (event, payload) => callback(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

contextBridge.exposeInMainWorld('electronAPI', {
  getAppState: () => ipcRenderer.invoke('get-app-state'),
  getConfig: () => ipcRenderer.invoke('get-config'),
  saveConfig: (config) => ipcRenderer.invoke('save-config', config),
  setActiveProvider: (protocolId, providerId) => ipcRenderer.invoke('set-active-provider', protocolId, providerId),
  getProxyStatus: () => ipcRenderer.invoke('get-proxy-status'),
  startProxy: () => ipcRenderer.invoke('start-proxy'),
  stopProxy: () => ipcRenderer.invoke('stop-proxy'),
  restartProxy: () => ipcRenderer.invoke('restart-proxy'),
  log: (...args) => ipcRenderer.send('log', args),
  platform: process.platform
});

contextBridge.exposeInMainWorld('electronEvents', {
  onConfigChanged: (callback) => subscribe('config-changed', callback),
  onProxyStatusChanged: (callback) => subscribe('proxy-status-changed', callback)
});
```

- [ ] **Step 2: 写 `electron/main.cjs`**

```js
// CCProxy 主进程：窗口 + 托盘 + IPC 边界 + 代理生命周期。
// 关窗口只隐藏不退出（代理要继续服务），退出统一走托盘菜单或 Cmd+Q。

'use strict';

const path = require('path');
const { app, BrowserWindow, ipcMain, dialog } = require('electron');
const logger = require('./logger');
const { createConfigManager, setActiveProvider } = require('./storage/config-manager');
const { createLocalProxy } = require('./services/proxy');
const tray = require('./tray');

// 未打包（开发态）一律加载 Vite dev server，改渲染层代码即时热更新；
// 生产走打包后的 dist/index.html。与 CCGUI 的加载决策一致
const DEV_SERVER_URL = 'http://127.0.0.1:5184';
const RENDERER_INDEX = path.join(__dirname, '..', 'dist', 'index.html');

let mainWindow = null;
let isQuitting = false;
let configManager = null;
let config = null;
let configIssues = [];
let configRecovered = false;
let configBackupPath = null;
let proxyStatus = { status: 'stopped', port: null, error: null };
let proxy = null;

function getState() {
  return { config, proxy: proxyStatus };
}

function broadcast(channel, payload) {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send(channel, payload);
  }
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1180,
    height: 780,
    minWidth: 940,
    minHeight: 620,
    title: 'CCProxy',
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: 16, y: 18 },
    backgroundColor: '#111315',
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.js')
    }
  });

  if (!app.isPackaged) {
    mainWindow.loadURL(DEV_SERVER_URL);
  } else {
    mainWindow.loadFile(RENDERER_INDEX);
  }

  // 关窗口只隐藏：代理要继续在后台服务
  mainWindow.on('close', (event) => {
    if (isQuitting) return;
    event.preventDefault();
    mainWindow.hide();
  });
  mainWindow.on('closed', () => {
    mainWindow = null;
  });
  return mainWindow;
}

function showWindow() {
  if (!mainWindow) createWindow();
  if (mainWindow.isMinimized()) mainWindow.restore();
  mainWindow.show();
  mainWindow.focus();
}

function setProxyStatus(next) {
  proxyStatus = next;
  tray.refresh();
  broadcast('proxy-status-changed', proxyStatus);
}

async function startProxy() {
  const next = await proxy.start().then(
    (info) => ({ status: 'running', port: info.port, error: null }),
    (error) => ({ status: 'error', port: config.settings.port, error: error.message })
  );
  setProxyStatus(next);
  logger.info('[Proxy] 启动结果', next);
  return next;
}

async function stopProxy() {
  await proxy.stop();
  setProxyStatus({ status: 'stopped', port: config.settings.port, error: null });
  logger.info('[Proxy] 已停止');
  return proxyStatus;
}

async function toggleProxy() {
  return proxyStatus.status === 'running' ? stopProxy() : startProxy();
}

// 所有配置变更的唯一出口：落盘 → 更新内存 → 刷托盘 → 通知界面
function applyConfig(nextConfig) {
  const saved = configManager.save(nextConfig);
  config = saved.config;
  configIssues = saved.issues;
  // 用户主动存过配置后，磁盘上已经是这份新配置，不再有「重建出来的那份」，
  // 重建横幅必须退场，否则后续的校验问题会被错并进「已重建」里
  configRecovered = false;
  configBackupPath = null;
  tray.refresh();
  broadcast('config-changed', {
    config,
    issues: configIssues,
    recovered: configRecovered,
    backupPath: configBackupPath || ''
  });
  return saved;
}

// 托盘与界面共用的供应商切换：语义在 config-manager 里，这里只管落盘与广播
function switchProvider(protocolId, providerId) {
  const next = setActiveProvider(config, protocolId, providerId);
  if (next === config) {
    logger.warn('[Config] 切换供应商被忽略（供应商不存在或未配置模型）', { protocolId, providerId });
    return { success: false, error: '该供应商未配置模型，无法启用' };
  }
  const saved = applyConfig(next);
  logger.info('[Config] 切换供应商', { protocolId, providerId });
  return { success: true, config: saved.config, issues: saved.issues };
}

// ============================================
// App Config IPC Handlers
// ============================================
ipcMain.handle('get-app-state', async () => {
  try {
    return {
      success: true,
      config,
      issues: configIssues,
      recovered: configRecovered,
      backupPath: configBackupPath,
      configPath: configManager.getPath(),
      proxy: proxyStatus
    };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('get-config', async () => {
  try {
    return { success: true, config };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('save-config', async (event, nextConfig) => {
  try {
    const saved = applyConfig(nextConfig);
    return { success: true, config: saved.config, issues: saved.issues };
  } catch (error) {
    logger.error('[Config] 保存失败', { error: error.message });
    return { success: false, error: error.message };
  }
});

ipcMain.handle('set-active-provider', async (event, protocolId, providerId) => {
  try {
    return switchProvider(protocolId, providerId);
  } catch (error) {
    logger.error('[Config] 切换供应商失败', { error: error.message });
    return { success: false, error: error.message };
  }
});

// ============================================
// Local Proxy IPC Handlers
// ============================================
ipcMain.handle('get-proxy-status', async () => {
  try {
    return { success: true, ...proxyStatus };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('start-proxy', async () => {
  try {
    const next = await startProxy();
    return { success: next.status === 'running', ...next };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('stop-proxy', async () => {
  try {
    const next = await stopProxy();
    return { success: true, ...next };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('restart-proxy', async () => {
  try {
    await stopProxy();
    const next = await startProxy();
    return { success: next.status === 'running', ...next };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

// ============================================
// Renderer Log IPC Handler
// ============================================
ipcMain.on('log', (event, args) => {
  logger.info('[Renderer]', args);
});

async function bootstrap() {
  logger.initialize();

  configManager = createConfigManager();
  const loaded = configManager.load();
  config = loaded.config;
  configIssues = loaded.issues;
  configRecovered = loaded.recovered;
  configBackupPath = loaded.backupPath;
  if (loaded.recovered) {
    logger.warn('[Config] 配置已损坏并重建', { backupPath: loaded.backupPath, reason: loaded.issues.join('；') });
  }
  // 首启与损坏重建时，拿到的默认配置只在内存里：必须落盘，否则下次启动还是走同一条分支，
  // 界面里显示的「配置文件」也始终不存在
  if (loaded.firstRun || loaded.recovered) {
    try {
      const saved = configManager.save(config);
      config = saved.config;
      configIssues = [...configIssues, ...saved.issues];
      logger.info('[Config] 已写入默认配置', { path: configManager.getPath(), firstRun: loaded.firstRun, recovered: loaded.recovered });
    } catch (error) {
      configIssues = [...configIssues, `默认配置写入失败：${error.message}`];
      logger.error('[Config] 默认配置写入失败', { path: configManager.getPath(), error: error.message });
    }
  }
  logger.info('[Config] 已加载', { path: configManager.getPath(), port: config.settings.port, issues: configIssues.length });

  proxy = createLocalProxy({
    getConfig: () => config,
    onLog: (entry) => logger.info('[Proxy] 请求', entry)
  });

  createWindow();
  tray.createTray({
    getState,
    actions: {
      switchProvider,
      openWindow: showWindow,
      toggleProxy,
      quit: () => app.quit()
    }
  });

  startProxy();
  app.on('activate', () => showWindow());
}

// 启动阶段抛错（窗口或托盘建不起来）时应用已经没法用了：报错并退出，
// 不留「代理在 listening 但没有窗口也没有托盘」的半初始化状态
function handleStartupError(error) {
  logger.error('[App] 启动失败', { error: error.message, stack: error.stack });
  dialog.showErrorBox('CCProxy 启动失败', error.message);
  app.quit();
}

// 单实例：代理占固定端口，且托盘应用多开会同时出现两个图标
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => showWindow());
  app.whenReady().then(bootstrap).catch(handleStartupError);
}

app.on('window-all-closed', () => {
  // 托盘应用：窗口全关也要让代理继续服务，这里什么都不做
});

app.on('before-quit', () => {
  isQuitting = true;
  tray.destroy();
  if (proxy) proxy.stop().catch(() => {});
});
```

- [ ] **Step 3: 校验主进程能起来（此时渲染层还没建，窗口会是白屏，属正常）**

应用是托盘应用、不会自己退出，所以放后台跑、看日志、再杀掉：

Run:
```bash
mkdir -p /tmp/ccproxy && cd /Users/alwaysking/AKProject/CCProxy
npx electron . > /tmp/ccproxy/m2a-smoke.log 2>&1 &
sleep 8
head -30 /tmp/ccproxy/m2a-smoke.log
```
Expected: 打印 `[Config] 已加载` 与 `[Proxy] 启动结果`，`status: 'running'`、`port: 18888`；无异常堆栈。

- [ ] **Step 4: 校验配置真的落盘了**

Run: `ls -l ~/.ccproxy/ && head -12 ~/.ccproxy/app.json`
Expected: 出现 `app.json`，内容含 `"port": 18888` 与 `anthropic`/`openai` 两个协议。**注意：这一步会创建 `~/.ccproxy/app.json`，属预期行为。**

- [ ] **Step 5: 校验端口确实在监听**

Run: `curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:18888/v1/messages -H 'content-type: application/json' -H 'anthropic-version: 2023-06-01' -d '{"model":"claude-sonnet-4-20250514","max_tokens":10,"messages":[]}'`
Expected: `404`（协议识别成功、但映射表还是空的），说明壳已经把 M1 的代理拉起来了。

- [ ] **Step 6: 退出应用**

Run:
```bash
pkill -f 'node_modules/electron/dist/Electron.app' || true
sleep 1
lsof -nP -iTCP:18888 -sTCP:LISTEN || echo '端口已释放'
```
Expected: 打印 `端口已释放`。

---

### Task 6: 渲染层骨架（样式 / 基础组件 / 应用壳 / stores）

照 CCGUI 的结构搬：`variables.css` 变量体系 + `components/base/` 原子组件 + Pinia setup-store 里直接调 `window.electronAPI`。**注意渲染层文件不写 `'use strict'`、不写分号**（跟 CCGUI 前端文件一致；本计划的「写分号」约定只适用于 Node 侧文件）——同一目录树里前端与 Node 的既有风格本来就不同，硬统一反而两边都不像。

**Files:**
- Create: `src/styles/variables.css`（从 CCGUI 复制）
- Create: `src/styles/global.css`
- Create: `src/utils/logger.js`
- Create: `src/utils/id.js`
- Create: `src/components/base/`（从 CCGUI 复制 10 个组件）+ `index.js`
- Create: `src/composables/useDialogStack.js`（从 CCGUI 复制，`BaseDialog.vue` 的依赖）
- Create: `src/main.js`
- Create: `src/App.vue`
- Create: `src/stores/useConfigStore.js`
- Create: `src/stores/useProxyStore.js`
- Create: `src/views/providers/ProvidersView.vue`

- [ ] **Step 1: 复制样式与基础组件**

从 CCGUI 复制下列文件（源路径 → 目标路径同名）：

| 源 | 说明 |
|---|---|
| `/Users/alwaysking/AKProject/CCGUI/src/styles/variables.css` | 整套 CSS 变量（深色主题，强调色 `#F97316`） |
| `.../CCGUI/src/components/base/BaseButton.vue` | 按钮 |
| `.../CCGUI/src/components/base/BaseInput.vue` | 输入框 |
| `.../CCGUI/src/components/base/BaseCard.vue` | 卡片 |
| `.../CCGUI/src/components/base/BaseDialog.vue` | 模态弹窗 |
| `.../CCGUI/src/components/base/Toggle.vue` | 开关 |
| `.../CCGUI/src/components/base/FormField.vue` | 表单字段包装 |
| `.../CCGUI/src/components/base/Badge.vue` | 徽章 |
| `.../CCGUI/src/components/base/EmptyState.vue` | 空状态 |
| `.../CCGUI/src/components/base/IconButton.vue` | 图标按钮 |
| `.../CCGUI/src/components/base/AppSelect.vue` | 下拉选择 |
| `.../CCGUI/src/composables/useDialogStack.js` | `BaseDialog.vue` 的依赖（Esc 关闭栈），必须一并复制，否则 barrel 导入整条链 500 |

- [ ] **Step 2: 检查复制过来的组件依赖是否可解析**

Run: `grep -rn "from '" src/components/base/ | grep -v "from 'vue"`
Expected: 剩下的每一条都要能在项目里找到落点：`@/` 开头走别名，`./`、`../` 开头要确认目标文件已存在。**注意不要把相对导入过滤掉** —— `BaseDialog.vue` 依赖的 `../../composables/useDialogStack` 正是相对路径，漏了它会让 `components/base/index.js` 整个 barrel 报 500、Vue 完全挂不上（页面白屏但控制台只有一条 `Failed to load resource: 500`）。

**若出现 CCGUI 专属依赖**（如 `composables/`、`utils/notifier`、`marked`、`highlight.js`）：
- 依赖链浅（一个 composable 或一个 util）→ 一并复制过来
- 依赖链深或引第三方库 → 不要为了搬运引入依赖，改为按同样的 props/emits 约定自写一个等价组件（保持 `modelValue` / `update:modelValue` 约定不变）

- [ ] **Step 3: 核对本阶段用到的组件 API，按实际 props 调整调用处**

下面 Step 11/12 的代码按 CCGUI 的通用约定写成（`Badge` 的 `variant`、`BaseButton` 的 `size`/`loading`、`BaseCard` 的默认插槽、`EmptyState` 的 `title`/`description`），但这些 props 的**实际取值要以复制进来的文件为准**。先确认：

Run: `grep -n "defineProps" -A 12 src/components/base/Badge.vue src/components/base/BaseButton.vue src/components/base/BaseCard.vue src/components/base/EmptyState.vue`
Expected: 拿到各自的 props 白名单与 `validator` 取值。若与下面代码里的用法不一致（例如 `Badge` 没有 `variant` 而是 `type`、可选取值不是 `success`），**改调用处去适配组件，不要改组件**——组件保持与 CCGUI 一致，以后才好同步。

- [ ] **Step 4: 建 `src/components/base/index.js`**

比 CCGUI 多导出 `AppSelect`（它那边是漏的，导致按路径 import 与按 index import 两种写法混用），这里统一从 index 导出。

```js
export { default as BaseButton } from './BaseButton.vue'
export { default as BaseInput } from './BaseInput.vue'
export { default as BaseCard } from './BaseCard.vue'
export { default as BaseDialog } from './BaseDialog.vue'
export { default as Toggle } from './Toggle.vue'
export { default as FormField } from './FormField.vue'
export { default as Badge } from './Badge.vue'
export { default as EmptyState } from './EmptyState.vue'
export { default as IconButton } from './IconButton.vue'
export { default as AppSelect } from './AppSelect.vue'
```

- [ ] **Step 5: 建 `src/styles/global.css`**

```css
@import './variables.css';

* {
  box-sizing: border-box;
}

html,
body,
#app {
  height: 100%;
  margin: 0;
}

body {
  background: var(--bg-primary);
  color: var(--text-primary);
  font-family: -apple-system, BlinkMacSystemFont, 'PingFang SC', 'Helvetica Neue', sans-serif;
  font-size: var(--font-size-md);
  -webkit-font-smoothing: antialiased;
}

::-webkit-scrollbar {
  width: 8px;
  height: 8px;
}

::-webkit-scrollbar-thumb {
  background: var(--border-default);
  border-radius: var(--radius-full);
}
```

若 `variables.css` 里实际的变量名与上面不同（如 `--font-size-md` 不存在），以复制进来的文件为准改这几处，不要新增变量。

- [ ] **Step 6: 建 `src/utils/logger.js`**

```js
// 渲染层日志：统一送主进程写文件，同时打到 devtools 控制台
function emit(level, message, data) {
  try {
    window.electronAPI?.log(level, message, data)
  } catch (error) {
    // 日志本身失败不能影响界面
  }
  const suffix = data === undefined ? '' : data
  if (level === 'error') console.error(message, suffix)
  else if (level === 'warn') console.warn(message, suffix)
  else console.log(message, suffix)
}

export const logger = {
  debug: (message, data) => emit('debug', message, data),
  info: (message, data) => emit('info', message, data),
  warn: (message, data) => emit('warn', message, data),
  error: (message, data) => emit('error', message, data)
}
```

- [ ] **Step 7: 建 `src/utils/id.js`**

与主进程 `config-manager` 的 `createConfigId` 保持同一形态（`前缀_8位随机串`）。

```js
// 生成配置条目 id：前缀 + 8 位随机串，与主进程 config-manager 的 createConfigId 形态一致
export function newId(prefix) {
  return `${prefix}_${crypto.randomUUID().replace(/-/g, '').slice(0, 8)}`
}
```

- [ ] **Step 8: 建 `src/stores/useConfigStore.js`**

```js
// 配置 store：唯一真源在主进程，本 store 只持有最近一次快照，
// 任何改动都先落本地副本再整体落盘，用主进程返回的归一化结果回填。
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

export const useConfigStore = defineStore('config', () => {
  const config = ref(null)
  const issues = ref([])
  const recovered = ref(false)
  const backupPath = ref('')
  const configPath = ref('')
  const isLoading = ref(false)
  const saving = ref(false)
  const error = ref(null)

  const protocols = computed(() => config.value?.protocols ?? [])

  function findProtocol(draft, protocolId) {
    return draft.protocols.find((item) => item.id === protocolId)
  }

  function applyState(state) {
    if (state.config) config.value = state.config
    if (state.issues) issues.value = state.issues
    if (typeof state.recovered === 'boolean') recovered.value = state.recovered
    if (typeof state.backupPath === 'string') backupPath.value = state.backupPath
    if (state.configPath) configPath.value = state.configPath
  }

  async function init() {
    isLoading.value = true
    error.value = null
    try {
      const result = await window.electronAPI.getAppState()
      if (!result.success) throw new Error(result.error)
      applyState(result)
    } catch (e) {
      error.value = e.message
      logger.error('加载配置失败', { error: e.message })
    } finally {
      isLoading.value = false
    }
  }

  async function save() {
    saving.value = true
    error.value = null
    try {
      const result = await window.electronAPI.saveConfig(config.value)
      if (!result.success) throw new Error(result.error)
      config.value = result.config
      issues.value = result.issues ?? []
      return true
    } catch (e) {
      error.value = e.message
      logger.error('保存配置失败', { error: e.message })
      return false
    } finally {
      saving.value = false
    }
  }

  // 所有改动都先改本地副本，立刻整体落盘；主进程归一化后的结果再回填，避免两边状态漂移
  async function mutate(recipe) {
    if (!config.value) return false
    const draft = JSON.parse(JSON.stringify(config.value))
    recipe(draft)
    config.value = draft
    return save()
  }

  function toggleProtocol(protocolId) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (protocol) protocol.enabled = !protocol.enabled
    })
  }

  function toggleProvider(protocolId, providerId) {
    return mutate((draft) => {
      const provider = findProtocol(draft, protocolId)?.providers.find((item) => item.id === providerId)
      if (provider) provider.enabled = !provider.enabled
    })
  }

  function toggleMapping(protocolId, mappingId) {
    return mutate((draft) => {
      const mapping = findProtocol(draft, protocolId)?.mappings.find((item) => item.id === mappingId)
      if (mapping) mapping.enabled = !mapping.enabled
    })
  }

  function upsertProvider(protocolId, provider) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      const index = protocol.providers.findIndex((item) => item.id === provider.id)
      if (index >= 0) protocol.providers[index] = provider
      else protocol.providers.push(provider)
    })
  }

  // 删供应商要连带删掉指向它的映射，不留悬空引用
  function removeProvider(protocolId, providerId) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      protocol.providers = protocol.providers.filter((item) => item.id !== providerId)
      protocol.mappings = protocol.mappings.filter((item) => item.providerId !== providerId)
    })
  }

  function upsertMapping(protocolId, mapping) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      const index = protocol.mappings.findIndex((item) => item.id === mapping.id)
      if (index >= 0) protocol.mappings[index] = mapping
      else protocol.mappings.push(mapping)
    })
  }

  function removeMapping(protocolId, mappingId) {
    return mutate((draft) => {
      const protocol = findProtocol(draft, protocolId)
      if (!protocol) return
      protocol.mappings = protocol.mappings.filter((item) => item.id !== mappingId)
    })
  }

  function updateSetting(key, value) {
    return mutate((draft) => {
      draft.settings[key] = value
    })
  }

  // 切换当前启用供应商：语义在主进程（要同步 * 兜底规则），这里只转发
  async function activateProvider(protocolId, providerId) {
    saving.value = true
    error.value = null
    try {
      const result = await window.electronAPI.setActiveProvider(protocolId, providerId)
      if (!result.success) throw new Error(result.error)
      config.value = result.config
      issues.value = result.issues ?? []
      return true
    } catch (e) {
      error.value = e.message
      logger.error('切换当前供应商失败', { protocolId, providerId, error: e.message })
      return false
    } finally {
      saving.value = false
    }
  }

  return {
    // State
    config,
    issues,
    recovered,
    backupPath,
    configPath,
    isLoading,
    saving,
    error,
    // Computed
    protocols,
    // Actions
    init,
    applyState,
    save,
    mutate,
    toggleProtocol,
    toggleProvider,
    toggleMapping,
    upsertProvider,
    removeProvider,
    upsertMapping,
    removeMapping,
    updateSetting,
    activateProvider
  }
})
```

- [ ] **Step 9: 建 `src/stores/useProxyStore.js`**

```js
// 代理状态 store：状态由主进程推来（proxy-status-changed），这里只做展示与转发操作
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { logger } from '@/utils/logger'

export const useProxyStore = defineStore('proxy', () => {
  const status = ref('stopped')
  const port = ref(null)
  const error = ref(null)
  const busy = ref(false)

  const isRunning = computed(() => status.value === 'running')
  const statusText = computed(() => {
    if (status.value === 'running') return '运行中'
    if (status.value === 'error') return '启动失败'
    return '已停止'
  })

  function apply(next) {
    if (!next) return
    status.value = next.status ?? 'stopped'
    port.value = next.port ?? null
    error.value = next.error ?? null
  }

  async function init() {
    try {
      const result = await window.electronAPI.getProxyStatus()
      if (result.success) apply(result)
    } catch (e) {
      logger.error('读取代理状态失败', { error: e.message })
    }
  }

  async function run(action) {
    busy.value = true
    try {
      const result = await action()
      if (result.status) apply(result)
      if (!result.success && !result.error) result.error = '操作失败'
      if (result.error) error.value = result.error
      return !!result.success
    } catch (e) {
      error.value = e.message
      logger.error('代理操作失败', { error: e.message })
      return false
    } finally {
      busy.value = false
    }
  }

  const start = () => run(() => window.electronAPI.startProxy())
  const stop = () => run(() => window.electronAPI.stopProxy())
  const restart = () => run(() => window.electronAPI.restartProxy())

  return {
    // State
    status,
    port,
    error,
    busy,
    // Computed
    isRunning,
    statusText,
    // Actions
    init,
    apply,
    start,
    stop,
    restart
  }
})
```

- [ ] **Step 10: 建 `src/main.js`**

```js
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles/global.css'

createApp(App).use(createPinia()).mount('#app')
```

- [ ] **Step 11: 建 `src/App.vue`**
顶部工具条（标题 + 页面入口 + 协议 Tabs + 代理状态）+ 顶部横幅 + 内容区。M2a 验收后的第六轮按设计文档去掉了左侧导航：设计里没有侧栏，协议 Tabs 在顶栏右侧，下方是供应商主页的两块（详见文末「第六轮」一节）。顶栏是拖拽区，右侧交互控件包在 no-drag 容器里。

```vue
<script setup>
// 应用壳：顶部工具条 + 内容区。
// 布局按设计文档：协议切换 Tab 在顶栏右侧，下方是供应商主页的两块（映射模块 + 代理商模块），没有左侧导航。
// 顶栏是拖拽区，右侧的按钮与 Tab 都在 no-drag 容器里，否则点不动。
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useConfigStore } from '@/stores/useConfigStore'
import { useProxyStore } from '@/stores/useProxyStore'
import { Badge, BaseButton, BaseTabs, EmptyState } from '@/components/base'
import ProvidersView from '@/views/providers/ProvidersView.vue'

// 页面入口。供应商是主页；日志 / 用量 / 设置按设计文档属 M3，先留入口和占位说明
const VIEW_ITEMS = [
  { key: 'providers', label: '供应商' },
  { key: 'logs', label: '请求日志', pending: '请求日志页在 M3 阶段实现：内存 500 条 + 落盘全量，可按协议与状态码过滤' },
  { key: 'usage', label: '用量统计', pending: '用量统计页在 M3 阶段实现：按天 / 供应商 / 模型聚合 tokens，模型级单价' },
  { key: 'settings', label: '设置', pending: '设置页在 M3 阶段实现：端口修改并重启代理、开机自启' }
]

const configStore = useConfigStore()
const proxyStore = useProxyStore()

const activeView = ref('providers')
const activeProtocolId = ref(null)

const currentView = computed(() => VIEW_ITEMS.find((item) => item.key === activeView.value))
const protocolTabs = computed(() => configStore.protocols.map((protocol) => ({ value: protocol.id, label: protocol.name })))

// 协议来自配置：配置被重建或协议列表变化后选中项可能失效，兜底回第一个，
// 免得两个模块拿到一个不存在的协议而渲染成空白
watch(
  () => configStore.protocols,
  (list) => {
    if (!list.some((item) => item.id === activeProtocolId.value)) activeProtocolId.value = list[0]?.id ?? null
  },
  { immediate: true }
)

// 顶部横幅：配置重建 / 校验问题 / 代理启动失败。
// 重建时原因本身就记在 issues 里，两者分开渲染会把同一件事说两遍，所以合并成一条。
const banners = computed(() => {
  const list = []
  if (configStore.recovered) {
    const reason = configStore.issues.length ? `：${configStore.issues.join('；')}` : ''
    const backup = configStore.backupPath ? `（备份：${configStore.backupPath}）` : ''
    list.push({ type: 'warn', text: `配置文件不可用，已重建为默认配置${reason}${backup}` })
  } else {
    for (const issue of configStore.issues) list.push({ type: 'warn', text: issue })
  }
  if (proxyStore.status === 'error') list.push({ type: 'error', text: `代理启动失败：${proxyStore.error}` })
  return list
})

async function onToggleProxy() {
  if (proxyStore.isRunning) await proxyStore.stop()
  else await proxyStore.start()
}

let unsubscribes = []

onMounted(async () => {
  unsubscribes.push(window.electronEvents.onConfigChanged((payload) => configStore.applyState(payload)))
  unsubscribes.push(window.electronEvents.onProxyStatusChanged((payload) => proxyStore.apply(payload)))
  await Promise.all([configStore.init(), proxyStore.init()])
})

onUnmounted(() => {
  unsubscribes.forEach((off) => off())
  unsubscribes = []
})
</script>

<template>
  <div class="app-shell">
    <header class="app-topbar">
      <span class="app-title">CCProxy</span>

      <div class="app-topbar-right">
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

        <BaseTabs
          v-if="activeView === 'providers'"
          v-model="activeProtocolId"
          :items="protocolTabs"
          size="sm"
        />

        <div class="app-proxy">
          <Badge :variant="proxyStore.isRunning ? 'success' : 'default'" size="sm">{{ proxyStore.statusText }}</Badge>
          <span class="app-proxy-port">{{ proxyStore.isRunning ? `127.0.0.1:${proxyStore.port}` : '未监听' }}</span>
          <BaseButton variant="secondary" size="sm" :loading="proxyStore.busy" @click="onToggleProxy">
            {{ proxyStore.isRunning ? '停止代理' : '启动代理' }}
          </BaseButton>
        </div>
      </div>
    </header>

    <main class="app-main">
      <div v-if="banners.length" class="app-banners">
        <div v-for="(banner, index) in banners" :key="index" class="app-banner" :class="`banner-${banner.type}`">
          {{ banner.text }}
        </div>
      </div>

      <ProvidersView v-if="activeView === 'providers'" :protocol-id="activeProtocolId" />
      <div v-else class="app-placeholder">
        <EmptyState :title="`${currentView.label}页待实现`" :description="currentView.pending" />
      </div>
    </main>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100%;
}

/* 全宽顶部工具条：整条可拖拽，红绿灯嵌在左侧（trafficLightPosition 18 正好落在 48px 高度的中线） */
.app-topbar {
  flex: 0 0 48px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--spacing-lg);
  padding: 0 var(--spacing-lg) 0 92px;
  -webkit-app-region: drag;
  border-bottom: 1px solid var(--border-default);
  background: var(--bg-secondary);
}

.app-title {
  font-size: var(--font-size-md);
  font-weight: var(--font-weight-semibold);
  color: var(--text-secondary);
}

/* 顶栏右侧整体不可拖拽：里面的按钮、Tab、下拉都要能接收点击 */
.app-topbar-right {
  display: flex;
  align-items: center;
  gap: var(--spacing-lg);
  -webkit-app-region: no-drag;
}

.app-view-nav {
  display: flex;
  align-items: center;
  gap: var(--spacing-xs);
}

.app-view-item {
  padding: var(--spacing-xs) var(--spacing-sm);
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.app-view-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.app-view-item.active {
  background: var(--bg-tertiary);
  color: var(--accent-primary);
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
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.app-banners {
  display: flex;
  flex-direction: column;
}

.app-banner {
  padding: var(--spacing-sm) var(--spacing-lg);
  font-size: var(--font-size-sm);
  border-bottom: 1px solid var(--border-default);
}

.banner-warn {
  background: var(--color-warning-bg);
  color: var(--color-warning);
}

.banner-error {
  background: var(--color-error-bg);
  color: var(--color-error);
}

.app-placeholder {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}
</style>
```

- [ ] **Step 12: 建 `src/views/providers/ProvidersView.vue`（最小可用版）**
供应商主页容器：按顶栏选中的协议渲染「映射模块 + 代理商模块」两块，模块实现在 `components/` 下（第六轮从最小可用版扩成了这个结构）。

```vue
<script setup>
// 供应商主页：按当前协议分两块——映射模块（客户模型名 → 供应商 / 真实模型）与代理商模块（供应商卡片）。
// 协议由顶栏 Tab 决定，本组件只负责把协议分发给两个模块，状态各自管各自的。
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

const configStore = useConfigStore()

const protocol = computed(() => configStore.protocols.find((item) => item.id === props.protocolId) || null)
</script>

<template>
  <div class="providers-page">
    <template v-if="protocol">
      <MappingModule :protocol="protocol" />
      <ProviderModule :protocol="protocol" />
    </template>

    <EmptyState
      v-else
      title="没有可用的协议"
      description="配置里没有协议条目，检查 ~/.ccproxy/app.json 或重启应用让配置重建"
    />
  </div>
</template>

<style scoped>
.providers-page {
  flex: 1;
  padding: var(--spacing-lg);
  display: flex;
  flex-direction: column;
  gap: var(--spacing-xl);
  overflow-y: auto;
}
</style>
```

- [ ] **Step 13: 起开发模式验收**

Run: `npm run electron:dev`
Expected:
- Vite 在 `127.0.0.1:5184` 起服务，Electron 窗口打开
- 左侧四项导航，「供应商」高亮
- 状态卡显示「运行中」与 `监听 127.0.0.1:18888`
- 点「停止代理」→ 徽章变「已停止」、按钮变「启动代理」；再点回来能恢复运行
- 托盘出现 ● 图标，菜单第一行是 `● 运行中 · 127.0.0.1:18888`，含 Anthropic / OpenAI 两个分组（都显示「（未配置供应商）」）
- 关闭窗口 → 窗口消失但进程还在（托盘图标仍在），点托盘「打开主窗口」能重新打开

- [ ] **Step 14: 校验配置文件路径与内容正确**

Run: `ls -l ~/.ccproxy/app.json && node -e "const c=require(process.env.HOME+'/.ccproxy/app.json'); console.log(c.settings.port, c.protocols.map(p=>p.id).join(','))"`
Expected: 打印 `18888 anthropic,openai`。

- [ ] **Step 15: 跑全量测试确认没被破坏**

Run: `npm test 2>&1 | tail -10`
Expected: `# pass 63`，0 失败。

---

## 本轮对 M1 代码的修复（预期外，验收反馈驱动）

`electron/services/proxy/index.js` 的 `start()` 在并发调用时会各自 `listen`，后一个失败时把共享的 `server` 置空，会抹掉前一个已经监听成功的引用——表现为「托盘和界面几乎同时点启动」时可能起两个监听、端口泄漏。修法是加一个 `starting` 句柄让并发调用复用同一次尝试。

同一处还有 `stop()` 的缺口：`start()` 在途时 `server` 还是 `null`，`stop()` 会立刻返回，随后那次 listen 落地就把服务留在运行态（界面显示已停止、端口却在监听）。修法是让 `stop()` 先等在途的 `starting` 有结果再关；启动失败的那次没有监听要关，吞掉即可，`stop()` 的契约是「让它停下来」。

`stop()` 认领了在途的那次启动之后，同一 tick 挤进来的 `start()` 仍会走 `if (starting) return starting` 复用它——而那是一次**正在被拆除**的尝试，于是 `start()` 会 resolve 出一个实际没人监听的端口。修法是引入 `stopping` 句柄：`stop()` 认领在途启动时把它记下，此后到达的 `start()` 先等停止收尾完，再新起一次监听。三处共补了四条回归测试（`tests/integration/proxy.test.js` 末尾），所以测试总数从 59 增至 63。

这是 M2a 实现过程中被验收发现的 M1 缺陷，改动范围仅限该函数，不属于 M2a 的新功能。

## 第六轮：界面布局回归设计（用户反馈驱动，M2b 供应商主页提前做完）

用户指出界面偏离原设计。根因不在渲染层，在计划：**Task 6 的「左侧导航」是计划自己发明的**，设计文档界面表写的是「顶栏协议 Tabs → 供应商卡片 → 映射表同页」，通篇没有侧栏。与用户确认后的目标布局：无侧栏、顶栏收纳全部（左侧标题拖拽区，右侧依次为页面入口、协议 Tabs、代理状态与启停）、两模块**映射在上、代理商在下**；设置页按设计文档里程碑属 M3，本轮只留入口与占位。

**改动清单**

- `src/App.vue` 重写：去掉侧栏；页面入口（供应商/请求日志/用量统计/设置）+ 协议 Tabs（仅供应商页显示，协议是映射与供应商的维度）+ 代理状态收进顶栏；`activeProtocolId` 由壳持有并下传，协议列表变化时兜底回第一个
- `src/views/providers/ProvidersView.vue` 变为两模块容器（接 `protocolId` prop）
- 新增 `src/views/providers/components/MappingModule.vue`（映射表：通配/兜底标识、启停、编辑、删除）与 `ProviderModule.vue`（供应商卡片：名称、baseUrl、模型数、当前启用蓝边、启停、编辑、删除）
- 新增 `src/views/providers/components/dialogs/ProviderDialog.vue`（名称/baseUrl/token/模型列表，token 可显隐，模型按行或逗号分隔）与 `MappingDialog.vue`（clientModel/目标供应商/真实模型 + 供应商模型快选）
- 新增基础组件 `BaseTabs.vue`（分段式 Tab，M3 日志页按协议过滤可复用）与 `ConfirmDialog.vue`（删除等不可逆操作统一走它）
- **当前启用蓝边用 `--color-info`**：设计文档指定蓝边，强调色橙留给主操作

**顺手修掉的潜伏 bug（M2a store，此前无界面路径触发）**：`useConfigStore.save()` 把 Vue 响应式代理直接送 IPC，structured clone 不支持 Proxy，报「An object could not be cloned.」，配置根本没落盘（界面还因乐观更新显示了假卡片）。根因修复：配置过 IPC 前统一经 `toPlain()` 转纯数据，`mutate()` 的 JSON 克隆也复用同一函数。

**行为要点（对齐主进程语义）**：点卡片 = `setActiveProvider`（同步 `*` 兜底规则）；没配模型的供应商主进程拒绝启用，原因显示在模块 notice；删供应商连带删映射，确认文案带映射条数；同一 clientModel 只允许一条（配置层只按 id 去重，重名条目的命中顺序说不清，直接在弹窗校验里挡掉）。

**验证**：CDP 驱动真实点击走完全流程——添加/编辑回填/启停/删除确认、点卡片切换当前启用（`*` 兜底跟随切换）、协议 Tab 切换数据隔离、占位页；配置逐次落盘核对（权限 600、无 .tmp 残留）；`npm test` 63 pass / 0 fail。

## 第七轮：视觉对齐 CC Switch（用户反馈驱动）

用户第二次反馈「界面非常丑陋」，并给出 CC Switch 自身的界面截图，要求「整体也参考一下 ccswitch 的界面设计」；配色基调定为对齐 CC Switch，但主色保留橙（不换蓝）。

**根因（三条，都是移植时丢的东西，不是某处 padding 没调好）**

1. **渲染层基线丢了**。`global.css` 缺 CCGUI 里那句 `* { margin: 0; padding: 0; box-sizing: border-box }`，浏览器默认 8px body margin 与各元素默认 margin 全在，块与块之间的间距忽大忽小、宽度也算不对；`body` 底色还错用了 `--bg-primary`——那是**弹窗面板**的颜色，不是画布色，于是顶栏看起来像凹进去一层、卡片和背景糊在一起分不出层级。
2. **顶栏三组元素等权**。标题、页面入口、协议 Tabs 视觉重量相当，读不出主次；且视图切换项 hover/active 也用橙，和协议 Tab 的橙一起在同一行里抢注意力（一屏两个橙色主色）。
3. **两个裸模块**。都是「标题 + 空盒子」，没有容器边界；空态用的是 Base 组件默认的 48px、50% 不透明度的占位图标（圆圈感叹号，看着像坏图），且头部主按钮与空态引导按钮同时出现，同一屏两个一模一样的橙按钮。

**改动清单**

- `src/styles/global.css` 重写：补回 `*` 归零与 `html/body/#app` 高度；画布色**硬编码** `#1D1D20`（和 CCGUI 里硬编码 `#111315` 同一习惯——画布色不是可换肤的 token，写死才不会被误改）；补 `:focus-visible` 焦点环、`::selection`、滚动条样式
- `src/styles/variables.css` 重写：灰阶整体换成 CC Switch 的（canvas `#1D1D20` / 面板 `#27272B` / 三级面 `#2C2C30` / 边框 `#3A3A40` / 次级文字 `#A1A1AA`，取自 `farion1231/cc-switch` 的 `src/index.css` 并做 HSL→hex 换算）；橙色强调色与语义色不动。**只改 token 值、不改任何基础组件**——token 的角色划分沿用 CCGUI，所以 9 个基础组件一行都没动。顺带删掉全项目已无引用的 `--sidebar-width` / `--sidebar-collapsed-width`（侧栏第六轮就没了）
- `electron/main.cjs` 的 `backgroundColor` 与 `index.html` 的内联底色跟着改成 `#1D1D20`：窗口底色、CSS 加载前的首屏底色、画布色三者必须一致，否则启动瞬间会闪一下别的颜色
- `src/App.vue`：顶栏改透明（露出画布色）+ 左侧留出红绿灯的拖拽区；标题改白字 15px 半粗（原先灰字 13px，像副标题）；页面入口改成「分段控件」样式，hover/active 走 `--bg-hover` + 白字，**橙色从这一组里撤掉**——顶栏只剩协议 Tab 一处橙，橙色从此专属于「当前协议」和主操作按钮；协议 Tabs 与页面入口之间加一条 1px 竖分隔线；代理按钮运行中降为 `ghost`（停止是次要操作，不该和主操作抢橙色）
- `src/views/providers/components/MappingModule.vue` 与 `ProviderModule.vue`：两个模块包成同款卡片（面板色 + 描边 + 圆角 + 头部一条分隔线，头部与内容同宽）；头部「+ 添加…」按钮加 `v-if`，只在列表非空时出现，避免和空态按钮重复；空态换成自定义 `#icon`（映射用「分支」——一个客户端模型名分出去指不同供应商；代理商用「服务器」——这块管的就是上游机器），并 `:deep()` 提亮图标和标题，否则在面板色卡片上太淡；映射表不再自带底色和圆角（在卡片里会两层灰叠一起），首末列去掉内侧 padding 与头部标题对齐
- `src/views/providers/components/ProviderModule.vue` 重写：供应商由卡片网格改成**行列表**（对齐 CC Switch 的供应商行）——首字方块头像 + 名称 + 等宽字体的 baseUrl + 模型 chip（超过 2 个折叠成 `+N`）+ 启停/编辑/删除；当前启用一行「蓝底蓝边 + 顶部 `当前启用` 徽章」（沿用第六轮的蓝=状态、橙=主操作的分工）；未配模型的行显示「未配置模型，无法设为当前启用」（主进程会拒绝启用，原因得先说出来）；行上所有按钮 `@click.stop`，点按钮不会连带触发「切换当前启用」
- `src/views/providers/ProvidersView.vue`：页面内边距与顶栏右内边距同档，卡片右边线和顶栏右端对齐

**没做的一件事**：拖拽手柄。CC Switch 的行首有拖拽手柄（可以给供应商排序），CCProxy 的配置里供应商没有排序语义，加了就是个假交互，不做。

**验证（不启动应用）**：`npm test` 63 pass / 0 fail；`npx vite build` 产物用临时静态服务（5199 端口，只读 dist，不碰 5184/18888）在浏览器里跑，注入假 `window.electronAPI` 喂配置，截图核对三种状态——有数据、空态、添加映射弹窗（弹窗面板 `#27272B` 比画布亮一档、输入框描边、保存为橙）；同时核对行为没坏：没有供应商时点「添加映射」不弹窗，只出「先去下面的代理商模块添加一个供应商」的提示。

## 第八轮：去掉内置兜底策略与「当前启用」（用户反馈驱动）

用户原话：「不要内置什么兜底策略了, 映射全部用户自己搞就行」。

**这套机制原本是什么**：协议里存一个 `activeProviderId` 当「当前启用」，点供应商行 / 点托盘菜单项 = 写它，并把该协议的 `*` 兜底规则指向这个供应商（没有就创建一条，`realModel` 取该供应商 `models[0]`）。第七轮的行内「当前启用」蓝边与徽章、第六轮的托盘 radio 子菜单，都是这套机制的两个落点。

**根因**：同一件事有两份状态。路由实际看的是 `*` 那行映射，而「当前启用」是另一份可写状态，两者靠 `setActiveProvider` 单向同步维持一致——于是用户手写一条 `*` 之后界面上的蓝边可能指错人，托盘里还能再切一次。更根本的是，用户根本不需要「一个当前供应商」这个抽象：路由规则本来就全在映射表里，一个协议下多条规则指向不同供应商是常态（`claude-*` 走甲、`*` 走乙）。内置兜底还顺手替用户写了一条他没写过的规则，等于代替用户做路由决策。

**改动清单（机制整体拆除，不是加开关）**

- `electron/storage/config-manager.js`：删 `FALLBACK_MODEL`、`getActivateBlockReason`、`isFallbackUsable`、`setActiveProvider` 与 `activeProviderId` 的派生逻辑；默认配置与 `buildProtocol` 的协议模板不再有该字段（配置里残留的会被忽略并丢弃，新增一条单测锁住这点）。`validateConfig` 仍是纯函数
- `electron/tray-menu.js`：删按协议分组的供应商 radio 子菜单；菜单回到「状态行 → 打开主窗口 / 启停代理 → 退出」
- `electron/main.cjs`：删 `switchProvider`、`set-active-provider` IPC、托盘 `switchProvider` 动作与相关 require
- `electron/preload.js`：删 `setActiveProvider` 桥
- `src/stores/useConfigStore.js`：删 `activateProvider`（走哪个供应商不再有这类落盘动作）
- `src/views/providers/components/ProviderModule.vue`：行不再可点（去 `onActivate`/`@click`/蓝边/徽章/hover 指针），描述改为「兜底走哪一个，由上面映射模块的 `*` 规则决定」；「未配置模型，无法设为当前启用」改成「未配置模型」
- `src/views/providers/components/dialogs/ProviderDialog.vue`：头部注释里「没有模型会被拒绝设为当前启用」的说法已不成立，改为说明模型列表的真实作用（不参与路由，只在映射弹窗里当快填 chip）
- 测试：`tests/unit/config-manager.test.js` 删四组 `activeProviderId` 派生用例与四组 `setActiveProvider` 用例，换成「残留字段不再保留」；`tests/unit/tray-menu.test.js` 删 radio 相关用例，换成「菜单里没有供应商切换项」

**没动的**：路由优先级本身（精确 > 通配 > `*`）与 `*` 的兜底语义仍在——变的是它现在只是一条用户手写的普通映射，不再是程序代管的东西；`provider.models` 字段保留（配置 schema 未变），只是它彻底变成 UI 快填用的数据，主进程从未读过它。

**验证（不启动应用）**：`npm test` 56 pass / 0 fail。

## 验收清单（M2a 整体）

执行完成后逐项确认，勾不上的必须先查清原因再报，不要含糊过去：

- [ ] `npm test` 全绿（56 条）
- [ ] `npm run electron:dev` 能起窗口，界面是深色主题、强调色橙色
- [ ] 配置写在 `~/.ccproxy/app.json`，权限 600，无 `.tmp` 残留
- [ ] 手工把 `app.json` 改成非法 JSON → 重启应用 → 提示「已重建为默认配置」且生成 `app.json.bak`，不崩
- [ ] 手工把端口改成已占用的值（如 5184）→ 重启 → 界面红色横幅显示 `EADDRINUSE`
- [ ] 托盘菜单只有状态行 / 打开主窗口 / 启停代理 / 退出四项（第八轮已拆掉供应商切换）
- [ ] 关窗口不退出；托盘「退出 CCProxy」能正常退出并释放端口
- [ ] 二次启动应用不会出现两个托盘图标（单实例锁）
- [ ] 项目目录里没有留下临时文件（图标脚本产物 `build/icons/` 属项目资源，保留）
- [ ] `/tmp/ccproxy/app.log` 有 JSON Lines 日志，能看到 `[Config] 已加载`、`[Proxy] 启动结果`

## 不在本阶段范围（M2b / M3）

- M2b：协议 Tabs、供应商卡片（当前启用蓝边）、供应商编辑弹窗、映射表与映射编辑弹窗、设置页（含端口修改并重启代理）
  - 第六轮已把前四项做完（见上）；设置页按设计文档里程碑属 M3
- M3：请求日志页（内存 500 条 + 落盘）、用量统计页（tokens 聚合 + 单价）、usage JSONL 埋点（design 文档第 6 步）、区分「客户端取消」与「上游失败」的埋点（M1 计划里的备忘）
- 打包（`electron:build`）与图标 `.icns`、CSP、响应侧 hop-by-hop 头剥离
