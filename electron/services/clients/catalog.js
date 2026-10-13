// codex 的模型清单（<codex 目录>/models.json）：期望条目的推导、与清单的比对、定点改写。
// codex 自带的清单编译在二进制里碰不着，用户级的模型能力全靠 config.toml 顶层的
// model_catalog_json 指到这份文件——所以检查/应用时这个文件和那个键要一起管。
// 这份文件是整份替换 codex 内置清单的，里面可能有别的来源的条目：归 CCProxy 管的
// 只有 slug 等于 OpenAI 协议某条映射的 clientModel 的那些，其余条目一个字节都不碰。
// 只依赖 fs/path 之外的标准库与 json.js，不 require electron，可脱离主进程单测。

'use strict';

const path = require('path');
const { readJsonObject, readJsonArray, planMembersAt, finish, indentAt, detectIndentUnit } = require('./json');

const CATALOG_FILE = 'models.json';
const CATALOG_KEY = 'model_catalog_json';

// codex 加载清单时逐条硬性要求的键（拿 codex debug models 逐个删键试出来的），
// 少一个整份文件都加载失败；条目还必须有 base_instructions 或
// model_messages.instructions_template 之一，否则语义校验也过不去
const REQUIRED_KEYS = [
  'slug',
  'display_name',
  'supported_reasoning_levels',
  'shell_type',
  'visibility',
  'supported_in_api',
  'priority',
  'support_verbosity',
  'truncation_policy',
  'experimental_supported_tools'
];
const INSTRUCTIONS_KEYS = ['base_instructions', 'model_messages'];

// 新建条目的兜底字段：清单里没有可抄的模板时，用这份最小集拼一个 codex 能接受的条目。
// 取值照 codex 自带清单的常规写法（shell_type 的 shell_command 会被 codex 归一成 unified_exec）
const NEW_ENTRY_DEFAULTS = {
  shell_type: 'shell_command',
  visibility: 'list',
  supported_in_api: true,
  priority: 1000,
  support_verbosity: false,
  truncation_policy: { mode: 'bytes', limit: 10000 },
  experimental_supported_tools: [],
  base_instructions: 'You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user\'s goals.'
};
// CCProxy 里勾了推理、清单里却没有档位可抄时用这组（与 codex 自带清单的写法一致）
const DEFAULT_REASONING_LEVELS = [
  { effort: 'none', description: 'Disable thinking' },
  { effort: 'low', description: 'Fast responses with lighter reasoning' },
  { effort: 'high', description: 'Enabled high reasoning' },
  { effort: 'max', description: 'Maximum reasoning depth for the hardest problems' }
];
const DEFAULT_REASONING_LEVEL = 'high';

function catalogFilePath(dirPath) {
  return path.resolve(dirPath, CATALOG_FILE);
}

// 文件不存在时从零起一份的骨架，后面照常走定点追加
function emptyCatalogText() {
  return '{\n  "models": []\n}\n';
}

// OpenAI 协议下「该出现在清单里」的条目：启用的映射（clientModel 不含 *——通配和兜底
// 是路由规则不是模型），名字用 clientModel，能力取映射指向的那个真实模型的配置——
// codex 看到的是 clientModel，实际跑的是 realModel，清单描述的得是真正干活的那位
function expectedCatalogEntries(protocol) {
  // 协议整体停用时代理根本不服务它，清单一条都不归我们管
  if (!protocol || protocol.enabled === false) return [];
  const providers = Array.isArray(protocol.providers) ? protocol.providers : [];
  const mappings = Array.isArray(protocol.mappings) ? protocol.mappings : [];
  const entries = [];
  for (const mapping of mappings) {
    if (!mapping || mapping.enabled !== true) continue;
    if (typeof mapping.clientModel !== 'string' || !mapping.clientModel || mapping.clientModel.includes('*')) continue;
    // 手编 app.json 能绕过界面对重名的拦截：同一个名字两条映射会生成两份期望，
    // 对同一条目改两遍会把数值写坏，只认第一条
    if (entries.some((entry) => entry.name === mapping.clientModel)) continue;
    const provider = providers.find((item) => item && item.id === mapping.providerId && item.enabled !== false);
    if (!provider) continue;
    const model = (provider.models || []).find((item) => item && item.name === mapping.realModel) || {};
    entries.push({
      name: mapping.clientModel,
      contextWindow: Number.isFinite(model.contextWindow) ? model.contextWindow : undefined,
      maxContextWindow: Number.isFinite(model.maxContextWindow) ? model.maxContextWindow : undefined,
      inputModalities: Array.isArray(model.inputModalities) && model.inputModalities.length ? model.inputModalities : undefined,
      reasoning: model.reasoning === true
    });
  }
  return entries;
}

// ============================================================================ 检查

function readCatalog(jsonText) {
  let parsed;
  try {
    parsed = JSON.parse(jsonText);
  } catch (error) {
    return { error: `模型清单不是合法 JSON：${error.message}` };
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return { error: '模型清单的根节点不是对象' };
  if (!Array.isArray(parsed.models)) return { error: '模型清单的 models 不是数组' };
  return { models: parsed.models };
}

function describe(value) {
  if (Array.isArray(value)) return value.join('/');
  return value === undefined ? '没配' : String(value);
}

// 一条归我们管的条目与期望的差：只比 CCProxy 配了的字段（没配的不拿去挑剔清单里的值），
// 推理比的是「有没有档位」——勾了推理就该有档位，没勾就不该有
function entryProblems(entry, want) {
  const problems = [];
  const slug = want.name;
  const missing = REQUIRED_KEYS.filter((key) => !(key in entry));
  if (missing.length) problems.push(`模型清单里 ${slug} 缺必需字段 ${missing.join('、')}，codex 会加载失败`);
  if (!INSTRUCTIONS_KEYS.some((key) => key in entry)) {
    problems.push(`模型清单里 ${slug} 缺 base_instructions，codex 会加载失败`);
  }
  if (want.contextWindow !== undefined && entry.context_window !== want.contextWindow) {
    problems.push(`模型清单里 ${slug} 的上下文窗口是 ${describe(entry.context_window)}，CCProxy 里配的是 ${want.contextWindow}`);
  }
  if (want.maxContextWindow !== undefined && entry.max_context_window !== want.maxContextWindow) {
    problems.push(`模型清单里 ${slug} 的最大上下文窗口是 ${describe(entry.max_context_window)}，CCProxy 里配的是 ${want.maxContextWindow}`);
  }
  if (want.inputModalities && JSON.stringify(entry.input_modalities) !== JSON.stringify(want.inputModalities)) {
    problems.push(`模型清单里 ${slug} 的输入模态是 ${describe(entry.input_modalities)}，CCProxy 里配的是 ${want.inputModalities.join('/')}`);
  }
  const levels = Array.isArray(entry.supported_reasoning_levels) ? entry.supported_reasoning_levels : [];
  if (want.reasoning && !levels.length) problems.push(`模型清单里 ${slug} 没有推理档位，CCProxy 里勾了推理`);
  if (!want.reasoning && levels.length) problems.push(`模型清单里 ${slug} 配了推理档位，CCProxy 里没勾推理`);
  return problems;
}

// 逐条比对：清单里缺我们的模型、条目能力与 CCProxy 配置不一致，都要报。
// 别的来源的条目不比能力也不改，但缺必需键会让整份清单加载失败，得说一声让人自己处理
function inspectCatalog(jsonText, expected) {
  const read = readCatalog(jsonText);
  if (read.error) return { status: 'unreadable', problems: [read.error] };

  const problems = [];
  const expectedNames = new Set(expected.map((entry) => entry.name));
  const seen = new Map();
  read.models.forEach((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      problems.push(`模型清单第 ${index + 1} 条不是对象，codex 加载这份清单会失败`);
      return;
    }
    const slug = typeof entry.slug === 'string' ? entry.slug : '';
    if (expectedNames.has(slug)) {
      seen.set(slug, (seen.get(slug) || 0) + 1);
      problems.push(...entryProblems(entry, expected.find((want) => want.name === slug)));
      return;
    }
    const missing = REQUIRED_KEYS.filter((key) => !(key in entry));
    if (!INSTRUCTIONS_KEYS.some((key) => key in entry)) missing.push('base_instructions');
    if (missing.length) {
      const label = slug || `第 ${index + 1} 条`;
      problems.push(`模型清单里 ${label} 缺必需字段 ${missing.join('、')}，codex 加载这份清单会失败；这条不归 CCProxy 管，请自行补齐`);
    }
  });
  for (const want of expected) {
    if (!seen.has(want.name)) problems.push(`模型清单里没有 ${want.name}（OpenAI 协议里启用了这条映射）`);
    else if (seen.get(want.name) > 1) problems.push(`模型清单里 ${want.name} 出现了 ${seen.get(want.name)} 次，codex 只认第一条`);
  }
  return { status: problems.length ? 'mismatch' : 'ok', problems };
}

// ============================================================================ 改写

function reasoningLevelsFrom(entry) {
  return Array.isArray(entry.supported_reasoning_levels) && entry.supported_reasoning_levels.length
    ? entry.supported_reasoning_levels
    : DEFAULT_REASONING_LEVELS;
}

// 已有条目要设成的键值：只含 CCProxy 配了的字段和补缺的必需键——没配的字段不写，
// 清单里已有的值可能是用户自己调的，不动
function entryValues(want, current) {
  const values = {};
  if (want.contextWindow !== undefined) values.context_window = want.contextWindow;
  if (want.maxContextWindow !== undefined) values.max_context_window = want.maxContextWindow;
  if (want.inputModalities) values.input_modalities = want.inputModalities;
  if (want.reasoning) {
    values.supported_reasoning_levels = reasoningLevelsFrom(current);
    if (current.default_reasoning_level === undefined) values.default_reasoning_level = DEFAULT_REASONING_LEVEL;
  } else {
    values.supported_reasoning_levels = [];
  }
  for (const key of REQUIRED_KEYS) {
    if (!(key in current)) values[key] = NEW_ENTRY_DEFAULTS[key];
  }
  if (!INSTRUCTIONS_KEYS.some((key) => key in current)) values.base_instructions = NEW_ENTRY_DEFAULTS.base_instructions;
  return values;
}

// 新建条目：抄模板的字段与排序风格（保留 base_instructions、priority 这些我们不在意的），
// 换掉身份与能力字段，缺的必需键用兜底值补
function buildNewEntry(want, template) {
  const entry = template ? JSON.parse(JSON.stringify(template)) : {};
  entry.slug = want.name;
  entry.display_name = want.name;
  entry.description = want.name;
  if (want.contextWindow !== undefined) entry.context_window = want.contextWindow;
  if (want.maxContextWindow !== undefined) entry.max_context_window = want.maxContextWindow;
  if (want.inputModalities) entry.input_modalities = want.inputModalities;
  entry.supported_reasoning_levels = want.reasoning ? reasoningLevelsFrom(template || {}) : [];
  if (want.reasoning && entry.default_reasoning_level === undefined) entry.default_reasoning_level = DEFAULT_REASONING_LEVEL;
  for (const key of REQUIRED_KEYS) {
    if (!(key in entry)) entry[key] = NEW_ENTRY_DEFAULTS[key];
  }
  if (!INSTRUCTIONS_KEYS.some((key) => key in entry)) entry.base_instructions = NEW_ENTRY_DEFAULTS.base_instructions;
  return entry;
}

// 多行块整体补一层缩进：第一行由插入点的逗号换行语法带着，后面的行自己缩
function indentBlock(block, indent) {
  return block.split('\n').map((line, index) => (index ? indent + line : line)).join('\n');
}

// 定点改写：缺的条目按模板补进数组末尾，已有的条目只改我们管的字段，
// 与我们重名的多余条目删掉，别的条目与全文格式一个字节不动。结构不对抛错，由调用方放弃写入
function rewriteCatalog(jsonText, expected) {
  const text = String(jsonText);
  const read = readCatalog(text);
  if (read.error) throw new Error(`${read.error}，为免写坏已放弃`);

  const rootStart = text.indexOf('{');
  const root = readJsonObject(text, rootStart);
  const modelsMember = root && root.members.find((member) => member.name === 'models');
  const array = modelsMember ? readJsonArray(text, modelsMember.valueStart) : null;
  if (!root || !modelsMember || !array) throw new Error('模型清单结构看不懂，为免写坏已放弃');

  const unit = detectIndentUnit(text);
  const items = array.items.map((span) => ({ span, object: readJsonObject(text, span.start) }));
  // 模板取第一个对象条目：新建条目抄它的字段与排序，跟清单自己的风格走
  const templateItem = items.find((item) => item.object);
  const template = templateItem ? JSON.parse(text.slice(templateItem.span.start, templateItem.span.end)) : null;

  const edits = [];
  const appended = [];
  for (const want of expected) {
    const matches = items.filter((item) => {
      if (!item.object) return false;
      const member = item.object.members.find((entry) => entry.name === 'slug');
      if (!member) return false;
      try {
        return JSON.parse(text.slice(member.valueStart, member.valueEnd)) === want.name;
      } catch {
        return false;
      }
    });
    if (matches.length) {
      const [hit, ...extra] = matches;
      const current = JSON.parse(text.slice(hit.span.start, hit.span.end));
      planMembersAt(text, hit.object, hit.span.start, entryValues(want, current), unit, edits);
      // 重名的多余条目删掉：codex 只认第一条，留着它检查永远收敛不到 ok。
      // 删除区间从上一个元素的尾到这个元素的尾，把中间那个逗号一并带走
      for (const dup of extra) {
        const index = items.indexOf(dup);
        edits.push({ start: items[index - 1].span.end, end: dup.span.end, text: '' });
      }
    } else {
      appended.push(buildNewEntry(want, template));
    }
  }

  if (appended.length) {
    if (items.length) {
      // 追加在末尾，照第一个条目的缩进对齐；几个新条目合成一次插入，不在同一位置打架
      const itemIndent = indentAt(text, items[0].span.start);
      const anchor = items[items.length - 1].span.end;
      const block = appended.map((entry) => indentBlock(JSON.stringify(entry, null, unit), itemIndent)).join(`,\n${itemIndent}`);
      edits.push({ start: anchor, end: anchor, text: `,\n${block}` });
    } else {
      const closeIndent = indentAt(text, array.closeIndex);
      const itemIndent = closeIndent + unit;
      const block = appended.map((entry) => indentBlock(JSON.stringify(entry, null, unit), itemIndent)).join(`,\n${itemIndent}`);
      edits.push({ start: modelsMember.valueStart + 1, end: modelsMember.valueStart + 1, text: `\n${block}\n${closeIndent}` });
    }
  }

  return finish(text, edits);
}

module.exports = {
  CATALOG_FILE,
  CATALOG_KEY,
  catalogFilePath,
  emptyCatalogText,
  expectedCatalogEntries,
  inspectCatalog,
  rewriteCatalog
};
