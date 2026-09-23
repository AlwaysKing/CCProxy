// 配置读写：~/.ccproxy/app.json。原子写（tmp + rename）、损坏自动备份 .bak 后回落默认配置，
// 不因配置问题崩溃。校验是纯函数，脱离 Electron 即可单测。

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

const CONFIG_VERSION = 1;
const DEFAULT_PORT = 18888;
// v1 只支持这两个协议：识别器与适配器都按这个集合工作，配置里出现别的协议没有意义
const SUPPORTED_PROTOCOLS = [
  { id: 'anthropic', name: 'Anthropic' },
  { id: 'openai', name: 'OpenAI' }
];

function createDefaultConfig() {
  return {
    version: CONFIG_VERSION,
    settings: { port: DEFAULT_PORT, launchAtLogin: false, pricing: {} },
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

// 单价：模型名 → { input, output }，单位是「每百万 tokens」。这笔数字要直接算出钱，
// 校验从严：认不出的条目丢掉并记 issue，绝不让 NaN 渗进费用计算
function normalizePricing(raw, issues) {
  if (!isPlainObject(raw)) return {};
  const pricing = {};
  for (const [model, price] of Object.entries(raw)) {
    const name = typeof model === 'string' ? model.trim() : '';
    if (!name) continue;
    if (!isPlainObject(price)) {
      issues.push(`模型 ${name} 的单价不是对象，已丢弃`);
      continue;
    }
    const input = Number(price.input);
    const output = Number(price.output);
    const inputValid = Number.isFinite(input) && input >= 0;
    const outputValid = Number.isFinite(output) && output >= 0;
    if (!inputValid && !outputValid) {
      issues.push(`模型 ${name} 的单价没有有效的输入/输出价格，已丢弃`);
      continue;
    }
    pricing[name] = { input: inputValid ? input : 0, output: outputValid ? output : 0 };
  }
  return pricing;
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
      pricing: normalizePricing(settings.pricing, issues)
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
  validateConfig,
  DEFAULT_PORT,
  CONFIG_VERSION
};
