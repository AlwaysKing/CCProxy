# CCProxy 项目默认模型设计

日期：2026-10-26
状态：已与用户逐节确认，待实施

## 一句话

CCProxy 增加可开关的“项目模型”功能：按项目工作目录统一管理 Codex 与 Claude Code 的默认
`clientModel`，在启用状态下保存绑定或映射变化时，自动写入各客户端的项目配置。

## 已锁定决策

| 项 | 决策 |
|---|---|
| 管理粒度 | 一个项目目录一行，分别设置 Codex 与 Claude Code 模型 |
| 职责边界 | 只写默认模型，不写 `base_url`、token、provider 或 Codex 模型清单 |
| Codex 目标 | `<项目>/.codex/config.toml` 顶层 `model` |
| Claude 目标 | Claude 实际使用的 `.claude/settings.local.json` 根节点 `model`；Git 子目录归到仓库根，worktree 归到主 checkout |
| 默认状态 | 功能默认关闭 |
| 关闭语义 | 保留绑定和已写入文件，只停止自动发现与后续同步 |
| 开启语义 | 对全部有效绑定执行一次同步 |
| 同步时机 | 保存绑定立即同步；协议映射变化后同步；无定时轮询 |
| 项目来源 | 自动发现与手动添加并存，合并后每个路径只出现一行 |
| 自动发现展示 | 未绑定项目直接进入列表，模型栏留空，不写任何文件 |
| 映射失效 | 不覆盖磁盘上的旧 `model`，绑定标为失效并停止该字段同步 |
| 共享 Claude 配置 | 多个项目解析到同一个 `settings.local.json` 时，只允许一个 Claude 绑定，避免后写覆盖前写 |
| 界面入口 | 顶部新增“项目模型”页 |
| 配置位置 | 继续使用 `~/.ccproxy/app.json` |

## 非目标

- 不在代理请求阶段读取或猜测当前工作目录。HTTP 请求没有可靠的标准化目录字段，而且同一
  客户端切换目录后可能继续复用会话。
- 不接管项目与 CCProxy 的连接配置。用户级客户端配置继续负责地址、token 和 provider。
- 不在关闭功能、清空绑定或映射失效时删除项目配置里已有的 `model`。
- 不自动删除项目文件，不恢复 `.ccproxy-bak`。
- 不对整个磁盘做项目扫描。

## 用户界面

顶部导航增加“项目模型”页。页面标题区包含：

- 功能总开关。
- 自动发现来源摘要。
- “刷新项目”和“添加项目”按钮。
- 搜索框和“显示已隐藏项目”选项。

项目列表一个项目一行，展示：

- 项目路径。
- 来源标签：Codex 自动发现、Claude 自动发现、手动添加。
- Codex 模型下拉框。
- Claude 模型下拉框。
- 同步状态与最近一次错误。
- 隐藏、删除等行操作。

自动发现但没有绑定的项目不写入 `app.json`。设置任一模型、手动加入或隐藏后才持久化该行，
避免历史项目永久污染配置。

模型候选由后端计算，不在前端自行拼接：

- Codex 读取 `openai_responses` 协议的映射。
- Claude Code 读取 `anthropic` 协议的映射。
- 只展示协议启用、供应商启用、模型组有效、`clientModel` 非空且不含 `*` 的精确映射。

保存某个模型字段后立即写入对应项目配置。两侧相互独立，任一侧留空不会影响另一侧。

## 配置模型

`CONFIG_VERSION` 升到 6。`settings` 增加：

```json
{
  "projectModelSyncEnabled": false,
  "projectModelProjects": [
    {
      "id": "p_xxx",
      "path": "/Users/example/code/project",
      "codexModel": "client-model-a",
      "claudeModel": "client-model-b",
      "manual": false,
      "hidden": false
    }
  ]
}
```

字段语义：

| 字段 | 语义 |
|---|---|
| `path` | 规范化后的绝对目录路径，也是项目唯一键 |
| `codexModel` / `claudeModel` | 对应协议中的精确 `clientModel`，允许为空 |
| `manual` | 是否为用户手动添加 |
| `hidden` | 是否从普通列表隐藏 |

读取旧配置时缺失字段回落为关闭、空列表。归一化规则：

- 配置层只接受绝对路径并做纯字符串规范化，不访问文件系统；目录是否暂时离线由运行态检查，
  不能让一次无关配置保存删掉离线项目的绑定。
- 按规范化路径去重，后者丢弃并产生 issue。
- 客户端模型为空或字段类型非法时只丢弃该值，不丢弃整行。
- 非法 ID 重新生成，并保证持久化项目 ID 全局唯一；非法条目只丢弃自身。

v5 到 v6 的迁移只新增项目模型设置，不重新排序映射。迁移提示需要与已有的 v4→v5 排序提示
分开，不能继续声称旧映射被重新整理。

## 自动发现

自动发现只在功能开启时运行，并在应用启动、进入页面及用户点击刷新时触发。它只读取客户端
自己的项目索引，不遍历用户主目录。

### Codex

扫描 `~/.codex/sessions/**/*.jsonl`。每个文件流式读取到第一条 `session_meta` 为止，使用
`payload.cwd` 作为项目路径；找到后不再读取正文。`payload.timestamp` 是会话开始时间，
因此最近活动时间统一取会话文件的 `mtime`，避免长会话和恢复会话排序失真。
遍历时跳过软链接文件和目录，避免历史目录里的链接造成循环或越界读取。

### Claude Code

优先流式读取 `~/.claude/history.jsonl` 的 `project` 与 `timestamp`。对缺少可用项目的历史版本，
回退扫描 `~/.claude/projects/**/*.jsonl`，读取事件中的 `cwd` 与 `timestamp`。Claude 历史里的
timestamp 可能是整数毫秒，项目事件里可能是 ISO 字符串；发现层统一转成 Unix 毫秒，解析失败
时回落到文件 `mtime`。

### 合并

- 路径先规范化，再按客户端来源合并，同一路径只生成一行。
- 不存在或不是目录的自动发现项跳过。
- 持久化绑定即使目录暂时消失仍保留，并显示“目录不存在”，不会被自动清除。
- 排序依次为：有绑定的项目、最近活动时间、路径。
- 被隐藏的自动发现项目仍可重新发现，但普通列表不显示。
- 自动发现遍历跳过软链接文件和目录。

## 写入引擎

新增 `clients/project_models` 后端模块，负责发现、合并、候选校验与写入。

现有客户端配置写入中的原子替换、软链接跟随、权限保留、`.ccproxy-bak` 备份和
“内容未变化不落盘”逻辑抽到共享写入辅助模块，由现有客户端目录应用和项目模型同步共用。

### Codex

只改项目根目录 `.codex/config.toml` 的顶层 `model`：

- 文件存在时只替换该键的值的字节区间，保留注释、键顺序、缩进、行尾及其他内容。
- 多次修改只动同一个值，不生成第二份 `model`。
- 文件不存在时创建 `.codex/` 和最小配置。
- 写入值使用合法的 TOML 基本字符串编码，并在替换后重新解析整个文件。
- 合法的单引号或三引号 `model` 通过完整 TOML 解析器定位，只替换值区间，不重新序列化整份
  文档；未闭合或整体不是合法 TOML 时拒绝写入并标为 `config_unreadable`。

### Claude Code

只改 Claude 实际作用域的 `.claude/settings.local.json` 根节点 `model`：

- 复用 JSON 局部编辑能力，保留缩进、键序、转义、行尾及其他字段。
- 文件不存在时创建 `.claude/` 和最小 JSON。
- 文件不是合法 JSON 时拒绝写入。
- Git 子目录按仓库根解析，worktree 按主 checkout 解析；多个项目解析到同一文件时只允许
  一个有效 Claude 绑定。

两个路径都使用临时文件加 rename 原子替换；目标是软链接时改写真实文件并保留链接。
项目模型属于自动写入，比手工“应用”多一道边界检查：目标文件或任一父目录的软链接解析后
必须仍位于对应项目作用域内；越界时拒绝写入并标为 `config_unreadable`。

## 同步流程

同步前先校验绑定：

1. 项目目录存在且可访问。
2. 对应协议、供应商、模型组和映射仍启用；模型组校验与代理路由器使用同一口径。
3. 绑定模型仍是该协议中的精确 `clientModel`。

校验通过后只写有值的一侧。同步规则：

- `set_project_model` 保存绑定后立即同步该项目。
- 功能从关闭切到开启时同步全部有效绑定。
- 协议、供应商或映射变化时同步全部仍有效的绑定。
- 功能关闭后不再触发项目配置写入。
- 所有项目持久化操作和同步任务使用 `AppState` 内的同一把配置事务锁，锁内完成“读取当前
  配置、修改、保存、同步”，避免 Tauri 命令并发执行时快照互相覆盖。

失效与错误不会触发删除或回滚：

| 状态 | 行为 |
|---|---|
| `ok` | 配置中的模型与期望值一致 |
| `sync_disabled` | 功能关闭；只保存绑定，不执行写入 |
| `binding_invalid` | 映射已停用、删除或不再精确存在；不写文件 |
| `project_missing` | 项目目录不存在；不写文件 |
| `config_unreadable` | 项目配置损坏或无权限；不写文件 |
| `write_failed` | 原子写失败；保留原文件并展示原因 |
| `restart_required` | 已写入；Codex app-server 运行中，需要重启后才读取新配置 |

Codex 与 Claude Code 两侧独立执行。任一侧失败不阻止另一侧成功，也不阻止其他项目继续同步。

已运行的客户端通常到新会话才读取新的默认模型。页面需要显示这个生效条件。
Codex 只加载受信任项目里的 `.codex/config.toml`，页面需要持续提示项目信任前提。CCProxy
新建的项目配置可能成为 Git 未跟踪文件，页面也需要提示提交前检查。

同步状态是运行时数据，不写入 `app.json`。`AppState` 按 `(projectId, client)` 保存最近状态、
时间与错误；项目或来源被删除时一并清理。Codex 的 daemon 检测读取 `CODEX_HOME`（未设置时为
`~/.codex`）下的全局 pid 文件，而不是项目下的目录。写入时若 daemon 正在运行，记录其 PID：

- 后续轮询发现 daemon 已停止或 PID 变化，`restart_required` 自动清除为 `ok`。
- 后台同步完成或状态变化后发送 `project-models-changed` 事件。
- `set_project_model` 直接返回本次状态；页面收到事件或配置变化后重新拉取完整视图。

## 后端接口

新增 Tauri command：

| 命令 | 作用 |
|---|---|
| `get_project_models` | 返回合并后的项目、模型候选与同步状态 |
| `refresh_project_models` | 刷新 Codex/Claude 项目索引并返回合并结果 |
| `set_project_model` | 保存一个项目某一客户端的模型并立即同步 |
| `add_project_dir` | 手动加入项目目录 |
| `hide_project` / `unhide_project` | 控制自动发现项目是否显示 |
| `remove_project` | 删除绑定；不删除项目文件 |

手动项目执行 `remove_project` 时删除持久化行；自动发现项目只删除绑定，之后仍由历史索引
发现，若要不再显示应使用 `hide_project`。所有持久化操作在配置事务锁内走
`AppState::apply_config`，继续使用唯一配置写入口、原子写和 `config-changed` 广播。

前端新增 `useProjectModelsStore`，只保存后端返回的合并视图和状态，不在渲染层推断路径、
候选模型或同步结果。

## 文件与模块

| 文件 | 改动 |
|---|---|
| `src-tauri/src/config.rs` | v6 配置字段、归一化、迁移与 issue |
| `src-tauri/src/clients/project_models.rs` | 发现、合并、候选校验、状态 |
| `src-tauri/src/clients/write.rs` | 抽出的共享原子写与备份辅助 |
| `src-tauri/src/clients/json.rs` | 支持根对象字段局部改写 |
| `src-tauri/src/clients/toml.rs` / `apply.rs` | 复用顶层 `model` 改写能力 |
| `src-tauri/src/state.rs` | 开启与映射变化时的同步副作用 |
| `src-tauri/src/commands.rs` / `lib.rs` | 新命令注册 |
| `src/views/projects/ProjectsView.vue` | 新页面 |
| `src/stores/useProjectModelsStore.js` | 页面状态与 action |
| `src/api/tauriBridge.js` | 新 IPC 方法 |
| `src/App.vue` | 顶部导航入口 |

## 测试

后端测试覆盖：

- Codex 与 Claude fixture 的自动发现、时间排序、来源合并和路径去重。
- 根 JSON / 顶层 TOML 改写，验证只改变目标字节并保留 CRLF、缩进、注释和权限。
- 文件不存在时创建最小配置；坏 JSON、坏 TOML、合法三引号 `model`、特殊字符编码、软链接、
  原子写与备份。
- v5 到 v6 配置迁移；坏项目条目被局部丢弃。
- 模型候选只来自正确的协议，并排除启用状态、供应商、模型组或通配符不合格的映射。
- 映射停用或删除时不覆盖旧模型。
- 一侧失败、另一侧成功；单个项目失败不影响其他项目。
- 保存绑定、开启功能和映射变化分别触发正确范围的同步；关闭功能不写文件。
- 目录消失时不删除绑定；配置事务并发保存不会丢失更新；隐藏与手动项目在重新发现后保持
  正确状态。
- Codex daemon 的 PID 变化后 `restart_required` 自动清除。

发布前验证：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
```

另用临时 HOME 和临时项目目录做一次端到端验证，确认 Codex 与 Claude 项目配置只写入
`model`，关闭功能后文件不再变化。

## 验收标准

- 开启功能后，项目列表能自动发现并手动加入 Codex/Claude 工作目录。
- 同一项目只出现一行，可分别设置两个客户端的默认模型。
- 保存模型后对应项目配置立即更新，其他配置内容不变。
- 映射变化会自动刷新仍有效的绑定；失效绑定只报警，不删除旧值。
- 关闭功能后绑定和文件均保留，且后续映射变化不再改项目文件。
- 现有客户端配置目录功能、代理路由和协议映射行为不受影响。
