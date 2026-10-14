# Electron → Tauri 迁移记录

日期：2026-10-14

## 一句话

壳（窗口 / 托盘 / IPC / 生命周期）换成 Tauri v2，代理与配置那套后端逻辑用 Rust 重写，
Vue 渲染层与 Vite 构建原样保留。安装包从 200MB 量级降到 10MB 量级。

## 为什么不是「Tauri 壳 + Node sidecar」

Tauri 里没有内嵌 Node。把原来的 Node 后端当 sidecar 塞进去，等于把 Node 运行时一起打包
（SEA 出来的单文件也有 80MB 上下），体积省得有限，还多出一个要自己管生命周期的子进程。
这个项目的后端本来就全是标准库能干的活（http、fs、json、toml 行扫描），用 Rust 重写一遍
才是真正把体积拿下来的做法，顺带把「渲染层跨进程拿数据」这件事从 Electron 的
contextBridge 换成了 Tauri 的 invoke / listen。

## 目录与模块对应

| 原（Electron / Node） | 现（Tauri / Rust） |
|---|---|
| `electron/main.cjs` | `src-tauri/src/lib.rs`（启动、单实例、窗口与托盘装配）＋ `state.rs`（配置变更的唯一出口） |
| `electron/preload.js` | `src/api/tauriBridge.js`（把 `window.electronAPI` / `window.electronEvents` 按原形状补上） |
| `electron/tray.js`、`tray-menu.js` | `src-tauri/src/tray.rs` |
| `electron/logger.js` | `src-tauri/src/logger.rs` |
| `electron/storage/config-manager.js` | `src-tauri/src/config.rs` |
| `electron/storage/request-log.js` | `src-tauri/src/request_log.rs` |
| `electron/services/proxy/*` | `src-tauri/src/proxy/*`（detector、router、adapters、upstream、models、fetch_models、server） |
| `electron/services/clients/*` | `src-tauri/src/clients/*`（dirs、toml、json、check、apply、catalog） |
| IPC 注册 | `src-tauri/src/commands.rs`（每个 `ipcMain.handle` 对应一个 `#[tauri::command]`） |

`src/`（Vue 渲染层）除下面两处外没有改动。

## 渲染层唯一的两处改动

1. `src/main.js` 挂载前调一次 `installTauriBridge()`，由 `src/api/tauriBridge.js` 把
   `window.electronAPI` / `window.electronEvents` 按原形状建出来（内部转发到 `invoke` / `listen`）。
   stores 与组件里那 20 多处 `window.electronAPI.xxx()` 因此一行都不用改。
   不在 Tauri 环境里（直接用浏览器开 `npm run dev`）时什么都不装，`?.` 自然变成空操作。
2. `src/App.vue` 顶栏加 `data-tauri-drag-region`：WKWebView 不认 `-webkit-app-region: drag`，
   拖拽窗口改由这个属性驱动（原 CSS 规则留着只当注释）。

## IPC 对应表

| 渲染层调用 | Rust 命令 |
|---|---|
| `getAppState` / `getConfig` / `saveConfig` / `regenerateClientToken` | `get_app_state` / `get_config` / `save_config` / `regenerate_client_token` |
| `getProxyStatus` / `startProxy` / `stopProxy` / `restartProxy` | `get_proxy_status` / `start_proxy` / `stop_proxy` / `restart_proxy` |
| `getClientDirs` / `pickClientDir` / `addClientDir` / `applyClientDir` / `removeClientDir` | `get_client_dirs` / `pick_client_dir` / `add_client_dir` / `apply_client_dir` / `remove_client_dir` |
| `fetchProviderModels` | `fetch_provider_models` |
| `getLogs` / `getUsage` / `getStorageStats` / `clearLogs` / `clearUsage` | `get_logs` / `get_usage` / `get_storage_stats` / `clear_logs` / `clear_usage` |
| `log` | `log` |

事件通道名不变：`config-changed`、`proxy-status-changed`、`log-appended`。

## 迁移中顺手修掉的两处旧 bug

1. **换客户端 token 后不再自动回写目录**。原来的 `wasOk` 过滤读的是 `entry.status`，
   而 `scanClientDirs` 返回的条目里根本没有 `status` 字段，所以那份名单恒为空，
   「原本配好的目录自动跟着写入新值」从来没生效过（注释里写明了这个意图）。
   现在按旧 token 先 `inspectClientConfig` 扫出真正 ok 的目录，换完 token 逐个回写。
2. **模型清单里缺 `display_name` 的条目会让写入直接失败**。`entryValues` 会给没有兜底值的
   必需键写一个 `undefined`，拼出来的 JSON 非法，最后在校验那步抛错。现在这类键跳过不写，
   检查页照样会把它报出来。

## 已知的行为差异（都不影响对外表现）

**非流式响应整体缓冲**。原来「不带响应回写」的非流式请求是边收边写的，现在统一先缓冲
再发（带整体超时）。换来的是埋点顺序与流式一致、代码少一半。LLM 的非流式响应是几 MB 级，
不影响使用。

**响应侧的 hop-by-hop 头会被剥掉**。`connection` / `transfer-encoding` 之类交给 hyper 自己
分帧，不再原样透传（透传会导致双重分帧）。业务头（`content-type`、`content-length`、
`content-encoding` 等）照旧。

**端口占用的报错文案**由 `listen EADDRINUSE` 换成「端口 18888 已被占用，请在设置里换一个端口」。

**开机自启**由 Electron 的 Login Item 换成 `tauri-plugin-autostart` 的 LaunchAgent，
用户可见效果一致。

**`get_client_dirs` 的内置条目多了个空 `id`**。渲染层是按 `path` 做 key 的，不受影响。

**托盘菜单**结构不变（状态行 → 打开主窗口 → 启停代理 → 退出），状态行文案与图标规则不变。

## 开发与打包

```bash
npm install
npm run tauri:dev      # 开发：Vite + Tauri 一起起（改渲染层热更新，改 Rust 自动重编译）
npm test               # 单元 + 端到端测试（cargo test）
npm run build          # 只构建前端
npm run tauri:build    # 打包 .app / .dmg（产物在 src-tauri/target/release/bundle/）
./scripts/rebuild.sh   # 打包 .app
./scripts/install.sh   # 装到 /Applications
```

版本号只有一处真源：`package.json` 的 `version`，`src-tauri/tauri.conf.json` 里的
`version` 指向它。发版改 `package.json` 再推 main，CI 会按 `v<version>` 建 Release。

## 单实例与「同时跑两个」

应用带单实例锁（macOS 上是 `/tmp/<bundle id>_si.sock`）：**已经有一个实例在跑时，再启动的
那个会立刻自己退出**，并把参数交给已在跑的那个（只把它窗口唤到前面）。所以「双击了但窗口一闪
就没了」= 已经有一个实例活着，不是崩溃。关窗口不退出同理：窗口只是隐藏，代理继续服务，
真要退出走菜单栏图标 → 退出 CCProxy。

调试时需要在正式实例照常开着的情况下另起一个，就带上开关并指到另一份配置（端口别撞）：

```bash
HOME=/tmp/ccproxy-dev CCPROXY_ALLOW_MULTI=1 \
  src-tauri/target/release/bundle/macos/CCProxy.app/Contents/MacOS/ccproxy
```

`CCPROXY_ALLOW_MULTI=1` 只是不上那把锁，其余行为不变；日常运行不需要它。

## 测试

原 17 个 `node:test` 文件（含端到端那套 mock 上游 + 真代理的集成测试）全部按同样口径
搬成了 Rust 测试，放在 `src-tauri/tests/`：detector、router、models、adapters、upstream、
config_manager、request_log、logger、tray_menu、client_toml、client_json、client_dirs、
client_check、client_catalog、client_apply、fetch_models、proxy_flow（端到端）。

