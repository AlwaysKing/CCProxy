# CCProxy 项目默认模型设计

日期：2026-10-26
状态：已与用户逐节确认，待按最终交互调整实现

## 一句话

在模型映射页按协议打开项目默认模型子窗口：Anthropic 管理 Claude 项目，OpenAI Responses
管理 Codex 项目；后台从全局会话目录中扫描一次候选，用户批量加入项目后自动维护对应客户端
配置里的顶层 `model`。

## 核心决策

| 项 | 决策 |
|---|---|
| 功能入口 | 模型映射页标题操作区增加项目图标 |
| 可见范围 | 仅 Anthropic 与 OpenAI Responses 显示；OpenAI Chat 不显示 |
| 管理界面 | 按当前协议打开独立子窗口，不做顶部 Tab、不占设置页 |
| 总开关 | 移除；功能常驻，但没有项目绑定时不写任何文件 |
| 数据粒度 | 一条绑定对应一个项目路径和一个协议/客户端模型 |
| 目录来源 | 后台扫描 Codex / Claude 全局会话目录，缓存为添加候选 |
| 候选刷新 | 启动时扫描一次；子窗口可手动刷新，不随页面切换重复扫描 |
| 添加流程 | 批量选择目录，并为这批目录选择一个初始模型 |
| 编辑流程 | 每个项目一行，可修改模型或移除绑定 |
| 移除语义 | 只删 CCProxy 绑定，不删除、不回滚项目配置 |
| 映射变化 | 自动刷新所有有效项目绑定 |
| 请求路径 | 不在代理请求中实时识别或写项目配置 |

## 非目标

- 不新增“项目模型”顶部导航。
- 不在设置页增加总开关或项目管理入口。
- 不支持 OpenAI Chat 的项目默认模型。
- 不把全局发现的目录自动加入项目列表；它们只作为添加候选。
- 不接管项目连接配置。base URL、token、provider 继续由用户级客户端配置负责。
- 不删除项目文件，不恢复 `.ccproxy-bak`。

## 用户界面

`MappingModule` 标题操作区按顺序显示协议 Tabs、项目图标按钮和“+ 添加映射”按钮。

项目图标：

- 仅在 `anthropic` 与 `openai_responses` 显示。
- 使用文件夹/项目类图标，悬停提示“项目默认模型”。
- 点击后打开按当前协议隔离的子窗口；重复点击时聚焦或重建为当前协议窗口。

子窗口标题显示“项目默认模型 · Claude”或“项目默认模型 · Codex”。内容只包含当前协议的绑定、
模型候选和目录候选，并提供“添加项目”和“刷新候选”。

每个项目一行，展示路径、默认 `clientModel`、同步状态与错误详情，可修改模型或移除绑定。

添加弹窗：

- 只展示当前协议对应来源的候选目录。
- 已在当前协议绑定的目录禁用。
- 支持多选。
- 选择一个初始模型，统一应用到所选项。
- 确认后逐个创建绑定并立即同步。

### 协议与来源

| 协议 | 客户端 | 会话候选来源 |
|---|---|---|
| `anthropic` | Claude Code | `~/.claude/history.jsonl`、`~/.claude/projects/**/*.jsonl` 中的工作目录 |
| `openai_responses` | Codex | `~/.codex/sessions/**/*.jsonl` 首条 session metadata 的 `cwd` |

候选使用规范化绝对路径并按客户端去重。扫描遍历跳过软链接目录，避免循环和越界读取。
候选和最终写入都拒绝文件系统根目录、用户主目录、`CODEX_HOME` 与
`CLAUDE_CONFIG_DIR`；否则会误改用户级默认配置而不是项目覆盖。

## 配置模型

配置版本升级到 v6，设置中保存扁平绑定列表：

```json
{
  "projectModelBindings": [
    {
      "id": "b_xxx",
      "path": "/absolute/project/path",
      "client": "codex",
      "model": "client-model-a"
    }
  ]
}
```

- `client` 只能是 `codex` 或 `claude`。
- 同一 `(path, client)` 只能有一条绑定。
- ID 必须全局唯一。
- 路径只做绝对路径的纯字符串归一化，不访问文件系统；暂时离线的项目保留配置。
- 模型为空、含通配符或字段类型非法时，该绑定归一化失败并产生 issue。

为兼容开发期间产生的中间配置，读取时若存在旧的 `projectModelProjects`，将其两个模型字段
展开成独立的 `client + model` 绑定，随后写出新结构。

## 目录与模型候选

`AppState` 增加运行时目录候选缓存，不写入 `app.json`：

- 应用启动时在后台扫描一次 Codex 与 Claude 会话目录。
- 首次打开子窗口时若缓存尚未完成，按需扫描并等待结果。
- “刷新候选”显式重新扫描。
- 扫描结果按客户端过滤，不互相混用。
- 扫描在阻塞任务中执行，单次刷新只允许一个任务；session 文件只读取首条 metadata，
  设置单行字节上限，解析失败、权限错误和超限文件跳过。

模型候选：

- Claude 使用 `anthropic` 协议映射。
- Codex 使用 `openai_responses` 协议映射。
- 只展示协议、供应商、模型组和映射均有效、`clientModel` 非空且不含 `*` 的精确名称。
- 候选去重并保持当前映射优先级顺序。

写入前模型必须能通过代理路由器解析。通配规则命中的具体模型允许绑定，但不进入精确候选列表。

## 写入与同步

只写客户端项目配置的顶层 `model`：

| 客户端 | 文件 |
|---|---|
| Codex | `<项目>/.codex/config.toml` |
| Claude Code | Claude 实际作用域的 `.claude/settings.local.json` |

Claude 的 Git 子目录归到仓库根，worktree 归到主 checkout。多个项目解析到同一个
`settings.local.json` 时拒绝创建第二条 Claude 绑定。

写入复用现有 JSON/TOML 局部编辑、原子替换、权限保留和 `.ccproxy-bak` 备份。文件不存在时
创建最小配置；自动写入的目标或父目录软链接解析后必须仍在项目作用域内。

TOML 先用完整解析器验证文件、顶层 `model` 类型和重复键，再做字节区间替换。扫描器必须把
数组表 `[[...]]` 与普通表都视为非顶层上下文，不能误改其中的 `model`；合法多行值只替换值区间，
歧义形态拒绝写入。

同步规则：

- 添加绑定或修改模型后，只立即同步受影响的绑定。
- 协议、供应商、模型组或映射变化后，同步全部有效绑定。
- 应用启动时同步全部有效绑定。
- 没有项目绑定时不执行同步。
- 项目持久化与同步使用同一把配置事务锁。
- 同步触发签名包含 protocols、providers、bindings；供应商启停或模型组变化也必须触发。

状态包括 `ok`、`binding_invalid`、`project_missing`、`config_unreadable`、
`write_failed`、`restart_required`、`requires_project_trust`。状态仅为运行时数据，
后台同步后发送 `project-models-changed` 事件。

每条扁平绑定都有自己的状态；同一路径的 Codex 与 Claude 绑定互不共享结果。

## 后端接口

| 命令 | 作用 |
|---|---|
| `get_project_models` | 按协议返回绑定、模型候选、目录候选和状态 |
| `refresh_project_models` | 重新扫描当前协议的目录候选 |
| `open_project_models_window` | 按协议打开或重建项目子窗口 |
| `add_project_bindings` | 批量添加目录并应用统一初始模型 |
| `set_project_binding_model` | 修改一条绑定的模型 |
| `remove_project_binding` | 移除绑定，不删除项目文件 |

命令参数使用 `protocol`，主进程映射为 `client`，避免前端重复维护协议到客户端的规则。

## 文件与模块

| 文件 | 改动 |
|---|---|
| `src-tauri/src/config.rs` | v6 扁平绑定、中间结构迁移、归一化 |
| `src-tauri/src/clients/project_models.rs` | 会话候选、模型候选、写入与状态 |
| `src-tauri/src/state/project_models.rs` | 候选缓存、绑定事务、同步与子窗口数据 |
| `src-tauri/src/commands.rs` / `lib.rs` | 新命令注册 |
| `src/views/providers/components/MappingModule.vue` | 标题操作区项目图标 |
| `src/views/projects/ProjectsView.vue` | 协议隔离子窗口 |
| `src/stores/useProjectModelsStore.js` | 当前协议子窗口状态 |
| `src/App.vue` | 识别子窗口路由，不增加顶部导航 |
| `src/views/settings/SettingsView.vue` | 不增加项目模型设置 |

## 测试与验收

- v6 归一化、中间结构迁移、重复路径和重复 ID。
- Codex / Claude 候选扫描、客户端过滤、路径规范化和缓存刷新。
- 跳过 `$HOME`、文件系统根目录和客户端配置根目录。
- 精确模型候选过滤、通配模型路由写入。
- 批量添加、模型修改、移除、同路径不同客户端绑定。
- JSON / TOML 写入、备份、软链接边界和坏文件保护。
- Claude Git 根、worktree 主 checkout 与同作用域冲突。
- 映射变化自动同步；无绑定不写文件。
- Codex 信任和 daemon 状态。
- 项目图标仅在支持的协议显示，子窗口按协议隔离。

发布前验证：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
```
