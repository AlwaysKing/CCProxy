// 配置读写：~/.ccproxy/app.json。原子写（tmp + rename）、损坏自动备份 .bak 后回落默认配置，
// 不因配置问题崩溃。校验是纯函数，脱离 Electron 即可单测。

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

// 版本号只标记写出的配置形态，加载时不比对：新版读旧配置是兼容的（模型条目两种写法都认），
// 但旧版读新配置不兼容（对象条目会被旧的字符串过滤静默滤掉），所以不承诺降级回滚
const CONFIG_VERSION = 1;
const DEFAULT_PORT = 18888;
// v1 只支持这两个协议：识别器与适配器都按这个集合工作，配置里出现别的协议没有意义
const SUPPORTED_PROTOCOLS = [
  { id: 'anthropic', name: 'Anthropic' },
  { id: 'openai', name: 'OpenAI' }
];

// 手动添加的客户端配置目录允许的类型；内置扫描位置不进配置（见 services/clients/dirs）
const CLIENT_DIR_KINDS = ['claude', 'codex'];

function createDefaultConfig() {
  return {
    version: CONFIG_VERSION,
    settings: { port: DEFAULT_PORT, launchAtLogin: false, logsEnabled: true, usageEnabled: true, clientDirs: [], clientToken: '' },
    protocols: SUPPORTED_PROTOCOLS.map((preset) => ({
      id: preset.id,
      name: preset.name,
      enabled: true,
      providers: [],
      mappings: []
    }))
  };
}

function createConfigId(prefix) {
  return `${prefix}_${crypto.randomUUID().replace(/-/g, '').slice(0, 8)}`;
}

// 客户端 token 是要填进客户端配置里的凭证，长得跟常见 API key 一致（sk- + 32 位十六进制），
// 与 createConfigId 生成的内部短 id 是两回事，别混用
function createClientToken() {
  return `sk-${crypto.randomBytes(16).toString('hex')}`;
}

function isPlainObject(value) {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

// 输入模态白名单：以 codex models.json 的 input_modalities 为准，界面也只给这两项勾
const INPUT_MODALITIES = ['text', 'image'];

// 模型条目：名称必填；上下文窗口与模态/推理能力只有 OpenAI 协议的界面会填，
// 这里不按协议区分——填了且合法就留，非法只丢该字段并记 issue，不连整个模型一起丢。
// 兼容旧格式：字符串条目直接当模型名（老配置里存的就是这种）
function normalizeModel(raw, protocolId, providerId, issues) {
  if (typeof raw === 'string') {
    const name = raw.trim();
    return name ? { name } : null;
  }
  if (!isPlainObject(raw)) {
    issues.push(`协议 ${protocolId} 的供应商 ${providerId} 下存在非法模型条目，已丢弃`);
    return null;
  }
  const name = typeof raw.name === 'string' ? raw.name.trim() : '';
  if (!name) {
    issues.push(`协议 ${protocolId} 的供应商 ${providerId} 下有模型缺少 name，已丢弃`);
    return null;
  }
  const model = { name };
  for (const field of ['contextWindow', 'maxContextWindow']) {
    const rawValue = raw[field];
    // 留空表示「没配」，不当成 0 存
    if (rawValue === undefined || rawValue === null || rawValue === '') continue;
    // 只认数字与纯数字串：true、[400000]、"1e5" 这些宽松强转出来的值一律拒掉
    const text = typeof rawValue === 'number' || typeof rawValue === 'string' ? String(rawValue).trim() : '';
    const value = /^\d+$/.test(text) ? Number(text) : NaN;
    if (Number.isInteger(value) && value > 0) model[field] = value;
    else issues.push(`模型 ${name} 的 ${field} 非法（${JSON.stringify(rawValue)}），已忽略`);
  }
  // null 与窗口字段一样当「没配」：手写的配置里 null 常常只是占位，不该弹警告
  if (raw.inputModalities !== undefined && raw.inputModalities !== null) {
    if (!Array.isArray(raw.inputModalities)) {
      issues.push(`模型 ${name} 的 inputModalities 不是数组，已忽略`);
    } else {
      const modalities = [...new Set(raw.inputModalities.filter((item) => INPUT_MODALITIES.includes(item)))];
      // 不强制 text 必须在：界面恒带上它，但手写的配置允许只声明 image
      if (modalities.length > 0) model.inputModalities = modalities;
      if (modalities.length < new Set(raw.inputModalities).size) {
        issues.push(`模型 ${name} 的 inputModalities 含不支持的取值，已丢弃`);
      }
    }
  }
  // OpenAI 的模型只有勾/不勾两态，没有「未配置」——没勾选就是显式 false（Anthropic 那边整个字段都不写）。
  // 这里非布尔不写回，是给手改配置兜底，不补默认值
  if (typeof raw.reasoning === 'boolean') model.reasoning = raw.reasoning;
  else if (raw.reasoning !== undefined && raw.reasoning !== null) issues.push(`模型 ${name} 的 reasoning 不是布尔值，已忽略`);
  return model;
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
  const models = [];
  const modelNames = new Set();
  for (const item of Array.isArray(raw.models) ? raw.models : []) {
    const model = normalizeModel(item, protocolId, raw.id, issues);
    if (!model) continue;
    if (modelNames.has(model.name)) {
      issues.push(`协议 ${protocolId} 的供应商 ${raw.id} 存在重复模型 ${model.name}，已丢弃后者`);
      continue;
    }
    modelNames.add(model.name);
    models.push(model);
  }
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

  return protocol;
}

// 客户端 token：客户端配置里填的必须是它，CCProxy 也拿它校验入站请求。
// 这里只做清洗不做生成——校验是纯函数，每次调用都换一个 token 就没法单测了；
// 缺失时由主进程启动阶段补一个并落盘
function normalizeClientToken(raw) {
  return typeof raw === 'string' ? raw.trim() : '';
}

// 手动目录条目：id/kind/path 齐全才留；path 必须是绝对路径——扫描按原样去找，
// 相对路径会随主进程工作目录漂移，存下来就是隐患
function normalizeClientDirs(raw, issues) {
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw)) {
    issues.push('settings.clientDirs 不是数组，已重置为空');
    return [];
  }
  const dirs = [];
  const seenPaths = new Set();
  for (const item of raw) {
    if (!isPlainObject(item)) {
      issues.push('settings.clientDirs 下存在非法目录条目，已丢弃');
      continue;
    }
    if (typeof item.id !== 'string' || !item.id) {
      issues.push('settings.clientDirs 下有目录条目缺少 id，已丢弃');
      continue;
    }
    const dirPath = typeof item.path === 'string' ? item.path.trim() : '';
    if (!dirPath || !path.isAbsolute(dirPath)) {
      issues.push(`目录条目 ${item.id} 的 path 要是绝对路径，已丢弃`);
      continue;
    }
    if (!CLIENT_DIR_KINDS.includes(item.kind)) {
      issues.push(`目录条目 ${item.id} 的 kind 非法（${JSON.stringify(item.kind)}），已丢弃`);
      continue;
    }
    if (seenPaths.has(dirPath)) {
      issues.push(`settings.clientDirs 存在重复目录 ${dirPath}，已丢弃后者`);
      continue;
    }
    seenPaths.add(dirPath);
    dirs.push({ id: item.id, kind: item.kind, path: dirPath });
  }
  return dirs;
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
      launchAtLogin: settings.launchAtLogin === true,
      // 开关缺省即开启：老配置里没有这两个键，读出来必须还是记录，行为不回退
      logsEnabled: settings.logsEnabled !== false,
      usageEnabled: settings.usageEnabled !== false,
      clientDirs: normalizeClientDirs(settings.clientDirs, issues),
      clientToken: normalizeClientToken(settings.clientToken)
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
  createClientToken,
  validateConfig,
  DEFAULT_PORT,
  CONFIG_VERSION
};
