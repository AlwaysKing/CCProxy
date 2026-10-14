# CCProxy 设计文档

日期：2026-09-21
状态：已与用户逐节确认

> 2026-10-14 补记：技术栈已从 Electron 换成 Tauri v2 + Rust 后端（本文描述的界面、配置结构、
> 请求管线与验收口径都没变）。壳与后端的模块对应关系见
> [Electron → Tauri 迁移记录](../../2026-10-14-electron-to-tauri.md)；
> 本文中 `electron/` 下的路径是迁移前的历史记录。

## 一句话

本地单端口 AI agent 协议代理：识别进来的请求属于哪个协议（Anthropic / OpenAI），按该协议的映射表把客户端模型名换成真实供应商 + 真实模型名 + 真实 token，转发上游并透传 SSE。界面仿照 CC Switch。Electron + Vue 3 + 纯 JS，分层习惯与 CCGUI 一致。

## 已锁定决策

| 项 | 决策 |
|---|---|
| 核心架构 | 协议适配器可插拔接口；第一版只实现「直通」适配器，跨协议转换留插入点不实现 |
| 协议识别 | 请求特征自动识别（路径 + 请求头 + 请求体打分），客户端 baseURL 不带前缀 |
| 配置结构 | 协议 → 供应商（名称/上游地址/token/模型列表）＋ 协议 → 映射表（clientModel → 供应商 + 真实模型名） |
| 认证方式 | 不暴露给用户 —— 由上游协议推导：Anthropic 发 `x-api-key`，OpenAI 发 `Authorization: Bearer` |
| 映射规则 | 未命中报错；支持通配（精确 > 通配 > 兜底）；协议/供应商/单条映射可独立启停 |
| 明确不做 | 供应商多把 token；第一版不做跨协议转换 |
| 界面布局 | 顶栏切协议 + 供应商卡片（当前启用）+ 映射表同页（方案 A，仿 CC Switch） |
| 技术栈 | Electron + Vue 3 + Vite + 纯 JS（对齐 CCGUI，复用其 local-proxy 思路与自建组件） |
| 托盘 | 要：图标含运行状态，菜单按协议分组切换供应商 |
| 首版范围 | 核心转发 + 配置界面 + 请求日志页 + 用量统计页 |
| 配置存放 | `~/.ccproxy/app.json`，原子写（tmp + rename） |
| 用量存放 | `~/.ccproxy/usage/YYYY-MM-DD.jsonl` 按天滚动 |
| 默认端口 | 18888，占用时报错不静默顺延 |
| git | 仅用户明确要求时才提交 |

## 项目结构

```
CCProxy/
├── package.json / vite.config.mjs / index.html
├── electron/
│   ├── main.cjs              # 入口：窗口 + 托盘 + IPC 注册 + 生命周期
│   ├── preload.js            # contextBridge 暴露 electronAPI
│   ├── logger.js             # 日志，落 /tmp/ccproxy/
│   ├── storage/
│   │   ├── config-manager.js # ~/.ccproxy/app.json 读写（原子写）
│   │   └── usage-store.js    # 用量 JSONL 按天滚动
│   └── services/proxy/       # 纯 Node，零 Electron 依赖，可独立测试
│       ├── index.js          # start/stop/restart/getStatus
│       ├── server.js         # http.createServer、读 body、分发
│       ├── detector.js       # 协议识别（打分制）
│       ├── router.js         # 映射查表：精确 > 通配 > 兜底
│       ├── adapters.js       # 协议适配器：认证注入 + model 改写位置
│       └── upstream.js       # 转发、SSE 透传、超时、错误映射
└── src/                      # Vue 3 渲染进程
    ├── stores/               # pinia：config / proxyStatus / logs / usage
    ├── components/base/      # 自建组件（从 CCGUI 搬）
    └── views/
        ├── providers/        # 主页：顶栏切协议 + 供应商卡片 + 映射表
        ├── logs/             # 请求日志
        ├── usage/            # 用量统计
        └── settings/         # 端口、开机自启等
```

## 请求处理管线

1. **入站**：记录 method + path + headers + body 概要（只记 model 与大小，不全文落盘）
2. **协议识别（detector.js）**：打分制
   - 路径：`/v1/messages*` → anthropic +3；`/v1/chat/completions`、`/v1/responses`、`/v1/completions`、`/v1/embeddings`、`/v1/models` → openai +3
   - 头：`anthropic-version` → anthropic +3；`x-api-key` → anthropic +1
   - body：顶层 `system` 且 `max_tokens` → anthropic +2
   - 并列或全零 → 400「无法识别协议」，body 附两种协议得分明细
3. **查表（router.js）**：读 body.model → 该协议映射表，精确 > 通配（最长前缀优先）> 兜底 `*`；逐层检查启停（协议停 → 503；供应商/映射停 → 该条不可用走下一优先级）；全未命中 → 404「模型 x 未在 y 协议映射表中配置」
4. **适配（adapters.js）**：按上游协议注入认证（anthropic → `x-api-key`；openai → `Authorization: Bearer`）；model 改写两协议均在 body 顶层；固定剥离入站 `authorization / x-api-key / api-key / x-goog-api-key / proxy-authorization` + hop-by-hop
5. **转发（upstream.js）**：baseUrl + 剩余路径（去重复 `/v1` 段）；SSE 边收边写；超时三档（首字节 60s / 流空闲 120s / 非流式 600s）；上游错误原样透传
6. **回传埋点**：anthropic 从 `message_start.usage` + `message_delta.usage` 累加，openai 从 `usage` 字段取；写 JSONL：时间、协议、clientModel、命中规则、目标供应商、真实 model、状态码、耗时、tokens

## 配置文件（~/.ccproxy/app.json）

```json
{
  "version": 1,
  "settings": { "port": 18888, "launchAtLogin": false },
  "protocols": [
    { "id": "anthropic", "name": "Anthropic", "enabled": true,
      "providers": [
        { "id": "p_zhipu", "name": "智谱", "enabled": true,
          "baseUrl": "https://open.bigmodel.cn/api/anthropic",
          "token": "sk-****",
          "models": ["glm-4.6", "glm-4.5-air"] } ],
      "mappings": [
        { "id": "m1", "enabled": true, "clientModel": "claude-sonnet-4-*",
          "providerId": "p_zhipu", "realModel": "glm-4.6" },
        { "id": "m9", "enabled": true, "clientModel": "*",
          "providerId": "p_zhipu", "realModel": "glm-4.6" } ] }
  ]
}
```

- **走哪个供应商只有映射表说了算**：没有 `activeProviderId` 这类「当前启用」状态，也没有内置兜底——`*` 兜底规则是用户自己写的一条普通映射，不写就不兜底（未命中报 404）
- 写入原子化：tmp + rename
- 启动时 schema 校验，损坏则备份 `.bak` 并提示，不崩

## 界面

| 页面 | 内容 |
|---|---|
| 供应商（主页） | 顶栏协议 Tabs → 映射表（clientModel / 目标供应商+真实模型 / 通配标识 / 启停 / 增删改）→ 供应商行（名称、baseUrl、模型编号、启停开关、编辑/删除）；`+` 弹窗添加供应商（名称、baseUrl、token、模型列表）。供应商行不可点：它不参与路由 |
| 请求日志 | 最近请求：时间、协议、clientModel → 命中规则 → 目标供应商/真实模型、状态码、耗时、tokens；按协议/状态码过滤；点开看识别打分明细；内存 500 条 + 落盘全量 |
| 用量统计 | 按天/供应商/模型聚合 tokens（输入/输出）、请求数、失败数；模型级可配单价，未配只显 tokens |
| 设置 | 端口（改完自动重启代理）、开机自启、日志清理 |
| 托盘 | 图标含运行状态点；菜单：端口状态、打开主窗口、启停代理、退出；关窗口不退出 |

## 错误处理

| 场景 | 行为 |
|---|---|
| 协议识别不出 | 400 + 得分明细 |
| 映射未命中 | 404 + 明确文案，界面提示 |
| 目标供应商停用 | 规则不可用走下一优先级，全不可用才报错 |
| 上游 4xx/5xx | 状态码 + body 原样透传 |
| 端口被占 | 启动失败红横幅，提示改端口 |
| 配置损坏 | 备份 `.bak` 并提示 |

## 测试与验收

- **单元测试**（node:test，/tmp 下跑，产物不进项目）：detector 打分（两协议真实样本）、router 优先级（精确/通配/兜底/停用跳过）、adapters 头剥离与注入
- **链路验收**：本地 mock 上游 + curl 模拟 Claude Code 与 OpenAI SDK 两种请求，验证改写、透传、SSE、用量提取、日志落盘
- 完成后派 subagent 验收；build 产物放 /tmp

## 里程碑

- **M1 代理核心**：services/proxy/ 六文件 + 单测，curl 可验全链路
- **M2 壳与配置**：Electron 壳（main/preload/托盘）+ 配置管理 + 供应商主页
- **M3 日志与用量**：埋点完善 + 日志页 + 用量页 + 设置页
