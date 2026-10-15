# CCProxy 协议转换设计

日期：2026-10-15
状态：已实施（2026-10-15）。方案 A 复用 + openai 拆分 + 供应商升为全局池

## 一句话

让客户端用自己协议的线格式发请求（Anthropic Messages / OpenAI Chat / OpenAI Responses），
代理按**供应商声明的上游格式**把请求转成上游要的样子、把响应转回客户端要的样子。
现在这条链路是直通的：上游必须说和客户端一样的协议。

## 1. 现状

- 协议识别只看路径，认两类：`/v1/messages*` → `anthropic`；`/v1/chat/completions`、
  `/v1/responses`、`/v1/completions`、`/v1/embeddings`、`/v1/models` → `openai`。
- 转发是直通：换掉 body 顶层的 `model`，路径原样接到 baseUrl 后面，响应只回写 model 名。
- 认证按**客户端**协议注入（anthropic 发 `x-api-key`，openai 发 `Authorization: Bearer`）。
- 设计文档里写死了「第一版不做跨协议转换」，这版把它补上。

## 2. 客户端线格式（按路径判，三条）

| 客户端格式 | 路径 | 上游端点（转发时按供应商格式改写） |
|---|---|---|
| `anthropic` | `/v1/messages*` | `/v1/messages` |
| `openai_chat` | `/v1/chat/completions` | `/v1/chat/completions` |
| `openai_responses` | `/v1/responses` | `/v1/responses` |

## 3. 核心决策

1. **协议条目拆成三条**：`anthropic` / `openai_chat` / `openai_responses`，各有一张映射表。
   老的 `openai` 条目在迁移时**复制**成 `openai_chat` 与 `openai_responses` 两条——旧行为下
   这两条路径都能走通，复制是唯一无损的迁移方式。
2. **供应商声明上游格式**：`apiFormat` 取 `anthropic` / `openai_chat` / `openai_responses`。
   转换方向 = 客户端格式 → 供应商格式；两边相同则透传（等价于现在的行为）。
3. **认证按供应商格式注入**，不再按客户端协议：`anthropic` 发 `x-api-key`（客户端没带
   `anthropic-version` 时补默认值），两个 openai 格式发 `Authorization: Bearer`。
4. **端点按供应商格式改写**：见第 2 节表。`/v1/messages/count_tokens` 等子路径单独处理。
5. **用量从转换后的响应提**：转换层会按目标协议的口径重排 usage（Anthropic 的
   `input_tokens` 不含缓存，OpenAI 的含），现有 `adapters::Usage` 口径不用改，也不会双算。
6. **转换引擎 vendor 进 `src-tauri/src/convert/`**：基线是 cc-switch `51ac86f1` 里已抽出的
   `api-format-proxy`（纯变换函数 + SSE 状态机 + 单测），去掉 Gemini 那套。
7. **不做**：Gemini 上游；供应商多把 token；上游故障转移。

## 4. 供应商放哪一层（已定：全局池）

拆成三个协议后，同一家上游要在多个协议下各建一遍，这是现在结构的直接后果。

| 方案 | 结构 | 代价 |
|---|---|---|
| A 全局供应商池（采用） | 供应商升为顶层列表，三个协议共用，各协议映射表按 id 引用 | 要迁移 app.json、要改界面 |
| B 仍按协议分组 | 保持现状，供应商只加 `apiFormat` 字段 | 同一上游（同 baseUrl/token）在多协议下要重复录入 |

采用 A：供应商是「一个上游端点 + 一种上游格式」，与客户端协议无关；拆成三个协议后，
按协议分组意味着同一家上游要录三遍。

## 5. 管线改动（src-tauri/src/proxy/）

请求侧，插在「换 model」之后：

```text
收体 → 模型列表接管 → 按路径定客户端格式 → 查映射表（该协议的）
  → 换 model → 按 (客户端格式 → 供应商格式) 转换 body
  → 按供应商格式改写端点、注入认证 → 转发
```

响应侧，替换现在只回写 model 名的 `ResponseRewriter`：

```text
上游响应 → 非流式：JSON → JSON 变换；流式：SSE 状态机
  → 统一把 model 写回 clientModel → 提用量 → 回客户端
```

要一起搬过来的配套件（cc-switch 的转换层已经把它们拆开了）：

- **工具上下文**：Responses → Chat / Anthropic 时，请求里被扁平化的工具名要在响应里还原，
  靠 `CodexToolContext` 从请求带到响应。
- **Codex 历史 store**：修多轮里 tool_call id 丢失的问题，进程内内存态，按会话 id 键。
- **SSE 分帧工具**：跨网络包拆分的 UTF-8 与 JSON 需要按块累积，不能按行猜。

## 6. 改动清单

| 文件 | 改动 |
|---|---|
| `config.rs` | `SUPPORTED_PROTOCOLS` 三条；`Provider` 加 `apiFormat`；老 `openai` 条目迁移；校验 |
| `proxy/detector.rs` | 路径 → 三种客户端格式 |
| `proxy/adapters.rs` | 认证与端点改按供应商格式；model 回写保留 |
| `proxy/server.rs` | 插入请求/响应转换与流式状态机 |
| `proxy/models.rs` | 模型列表按客户端格式出（一行代码的口径调整） |
| `state.rs` | codex 的 `models.json` 从 `openai_responses` 那条协议取（现在取的是 `openai`） |
| 前端 | 三个协议 tab；供应商多一个「上游格式」选择器；模型属性行按格式显示 |

## 7. 待定行为

- **`/v1/messages/count_tokens`**：现在原样转发，上游一旦不是 Anthropic 就转不了。
  倾向本地按字符数估算返回（Claude Code 只拿它做上下文预估，不精确可接受）。
- **`/v1/completions`、`/v1/embeddings`**：现在归在 openai 协议下，拆分后归 `openai_chat`。

## 8. 验收

- vendor 过来的转换单测（`cargo test`）全绿。
- 新增测试：三条路径的识别、端点改写、认证注入、三个转换方向的非流式与流式端到端
  （本地 mock 上游）。
- 手工：Claude Code → OpenAI Chat 上游、codex → Anthropic 上游各跑一轮真实对话，
  含工具调用与流式。

## 9. 已知风险

- 流式状态机是最容易出错的地方（工具参数分段、usage 收尾、截断流）。
- 拆分协议后，同一个客户端模型名要在 `openai_chat` 与 `openai_responses` 两张表里各配一条，
  这是拆分的固有成本。
- 上游返回 2xx 但语义失败（错误 envelope、`status=failed`）必须转成真错误，
  不能变成静默的空回复。

## 10. 实施结果（2026-10-15）

已落地：

- **转换引擎**：`src-tauri/src/convert/`，从 cc-switch `51ac86f1` vendor（自带 739 个单测，
  全部通过）。去掉的是 Gemini 那套模块、usage 下的计费与日志（依赖数据库）、应用层的类型。
- **配置**：协议三条（`anthropic` / `openai_chat` / `openai_responses`）；供应商升为顶层
  `providers` 并带 `apiFormat`（`auto` | 三种格式）；老配置自动迁移——供应商提升到顶层、
  格式按归属协议推断（anthropic → `anthropic`，老的 openai 那条取 `auto`，行为与迁移前一致）、
  老的 `openai` 映射复制到拆出来的两条协议上。
- **识别**：路径 → 三种线格式；`/v1/completions`、`/v1/embeddings` 归 `openai_chat`。
- **转发**：端点按上游格式改写、认证按上游格式注入（Anthropic 缺 `anthropic-version` 时补默认值）、
  请求转换 → 转发 → 响应回转（含 SSE 状态机）、用量从转换后的响应体提。
- **`/v1/messages/count_tokens`**：上游不是 Anthropic 时本地按字符数估算返回，不转发。
- **界面**：三条协议 Tab；供应商模块变成全局池（行上带上游格式徽章）；供应商弹窗多一个
  「上游格式」选择；映射弹窗的供应商下拉改读全局池；模型的能力字段只在「会被 responses 侧
  用到」时显示。
- **测试**：端到端用例覆盖 Anthropic ↔ Chat（非流式与流式）、Responses ↔ Anthropic、
  count_tokens 本地应答、以及不支持的方向要响亮失败（不回退成透传）。

### 响应侧的收尾（同日补完）

转换层只管协议结构，但真实上游不总是按 `stream` 参数办事，这几种情形单独处理了：

| 情形 | 处理 |
|---|---|
| 上游报错（非 2xx） | 错误体换算成客户端那一套形状（Anthropic `{"type":"error","error":{...}}`、OpenAI 一族 `{"error":{...}}`），状态码保留；认不出文案时原样透传，不编失败原因 |
| 客户端要流式、上游回了整块 JSON | 先转成客户端格式，再展开成完整 SSE 生命周期（`convert/stream.rs` 的 `json_response_to_sse`） |
| 2xx 里带 error envelope | 不展开：原样交给客户端，绝不伪造「成功但空」的 SSE |
| 客户端要整块、上游回了 SSE | 原样透传（不做聚合），比编一个 502 诚实 |

### 明确不做

- **客户端是 OpenAI Chat 时的两对转换**（chat → anthropic、chat → responses）：客户端实际只有
  Claude Code（Anthropic）与 Codex（Responses）两种，用不到，不做。
- 上游对「客户端要整块」却回 SSE 时的聚合：透传即可，聚合要另写一套 SSE→JSON 的解析。
- 供应商级的 reasoning 参数风格（`thinking` / `reasoning_effort` / OpenRouter 形态）目前用
  转换层按模型名的启发式判断，没有做成供应商上的配置项。
