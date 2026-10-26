# CCProxy

本地单端口 AI agent 协议代理。识别进来的请求属于哪种线格式（Anthropic Messages /
OpenAI Chat / OpenAI Responses），按该协议的映射表把客户端模型名换成真实供应商 +
真实模型名 + 真实 token，再按**映射所属模型组的上游格式**转换协议、转发上游，
把响应转回客户端要的形状。界面仿 CC Switch。技术栈：Tauri v2 + Rust（壳与后端）＋
Vue 3 + Vite（界面）。

## 它解决什么

Claude Code / Codex 这类客户端只认自己的协议和模型名。想让它们跑别的供应商（智谱、DeepSeek、
Kimi……），传统做法是改客户端的 base_url 与 key——每个客户端各配一遍，模型名还得对齐。
CCProxy 把这些收在一处：

- 客户端只填 `http://127.0.0.1:18888` 和一个假 token，真实凭据不进客户端配置
- 三件事都在界面里配，改完即时生效：供应商（共用地址、token，模型分 Claude / OpenAI 两类）、
  协议 → 映射表（clientModel → 供应商 + 上游协议 + realModel）、客户端与上游协议决定要不要转换
- **协议互转**：客户端说 Anthropic、供应商只认 OpenAI 也能通。模型列表只分
  Claude / OpenAI；每条映射明确选择 Anthropic、OpenAI Chat 或 OpenAI Responses 上游协议
- 供应商是全局池：同一家上游配一次，地址与 token 共用，三个协议的映射表按供应商 + 模型组引用
- 请求日志与用量统计分两页：tokens 按天 / 协议 / 供应商 / 模型聚合
- 项目模型：可开关，自动发现或手动添加工作目录，按项目分别固定 Codex / Claude Code
  的默认模型；只写项目配置里的 `model`，关闭后保留已有绑定和文件
- codex 的 `models.json` 模型清单跟着映射表自动刷新，不用手工维护；codex 的 app-server daemon
  只在启动时读一次 `config.toml` 与模型清单，改完设置页会提醒执行 `codex app-server daemon restart`

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

产物是 ad-hoc 签名、未公证，且只编了 arm64。**对方拿到后第一次打开会被 macOS 拦一次**：
从浏览器/网盘/微信下来的文件带着 quarantine 标记，系统认不出签名者。放行方式三选一：

- 系统设置 → 隐私与安全性 → 底部「仍要打开」（macOS 15 及以后）
- 右键点 App → 打开（老版本 macOS）
- 终端去掉标记：`xattr -dr com.apple.quarantine /Applications/CCProxy.app`

不确定标记在不在，先看一眼：`xattr -p com.apple.quarantine /Applications/CCProxy.app`，
有输出才需要上面第三步。

只编了 arm64，Intel Mac 跑不了；要免掉「首次放行」这步得做 Developer ID 签名 + 公证。
细节见[迁移记录的分发那节](docs/2026-10-14-electron-to-tauri.md)。

## 文档

- [设计文档](docs/superpowers/specs/2026-09-21-ccproxy-design.md)：协议识别、映射查表、转发管线、错误处理的口径
- [Electron → Tauri 迁移记录](docs/2026-10-14-electron-to-tauri.md)：模块对应、IPC 对应、行为差异、分发与 CI 注意事项
