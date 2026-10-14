# CCProxy

本地单端口 AI agent 协议代理。识别进来的请求属于哪个协议（Anthropic / OpenAI），按该协议的
映射表把客户端模型名换成真实供应商 + 真实模型名 + 真实 token，转发上游并把 SSE 原样透传。
界面仿 CC Switch。技术栈：Tauri v2 + Rust（壳与后端）＋ Vue 3 + Vite（界面）。

## 它解决什么

Claude Code / Codex 这类客户端只认自己的协议和模型名。想让它们跑别的供应商（智谱、DeepSeek、
Kimi……），传统做法是改客户端的 base_url 与 key——每个客户端各配一遍，模型名还得对齐。
CCProxy 把这些收在一处：

- 客户端只填 `http://127.0.0.1:18888` 和一个假 token，真实凭据不进客户端配置
- 协议 → 供应商 → 映射表（clientModel → 供应商 + realModel）都在界面里配，改完即时生效
- 请求日志与用量统计分两页：tokens 按天 / 协议 / 供应商 / 模型聚合
- codex 的 `models.json` 模型清单跟着映射表自动刷新，不用手工维护

## 跑起来

```bash
npm install
npm run tauri:dev      # 开发：Vite + Tauri 一起起（改界面热更新，改 Rust 自动重编译）
npm test               # 单元 + 端到端测试（cargo test）
npm run tauri:build    # 打包 .app / .dmg（产物在 src-tauri/target/release/bundle/）
./scripts/rebuild.sh   # 只打包 .app
./scripts/install.sh   # 装到 /Applications
```

## 东西放在哪

| 内容 | 位置 |
|---|---|
| 配置 | `~/.ccproxy/app.json`（原子写；损坏时备份 `.bak` 再回落默认值） |
| 请求日志、用量 | `~/.ccproxy/logs/`、`~/.ccproxy/usage/`（按天 JSONL，清理互不影响） |
| 运行日志 | `/tmp/ccproxy/app.log` |

关窗口不退出（代理要继续在后台服务），退出走菜单栏图标 → 退出 CCProxy。

## 分发

产物是 ad-hoc 签名、未公证，且只编了 arm64：对方第一次打开要放行一次，Intel Mac 跑不了。
细节见[迁移记录的分发那节](docs/2026-10-14-electron-to-tauri.md)。

## 文档

- [设计文档](docs/superpowers/specs/2026-09-21-ccproxy-design.md)：协议识别、映射查表、转发管线、错误处理的口径
- [Electron → Tauri 迁移记录](docs/2026-10-14-electron-to-tauri.md)：模块对应、IPC 对应、行为差异、分发与 CI 注意事项

