// 配置管理单测：默认配置、原子写、损坏回落、校验归一化
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const {
  createConfigManager,
  createDefaultConfig,
  validateConfig,
  DEFAULT_PORT
} = require('../../electron/storage/config-manager');

// 每个用例一个临时目录（写在 /tmp 下，不污染项目）
function makeManager(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-cfg-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return { dir, manager: createConfigManager({ dir }) };
}

function provider(id, models = ['glm-4.6']) {
  return { id, name: id, enabled: true, baseUrl: `https://${id}.example.com`, token: 'sk-x', models };
}
function mapping(id, clientModel, providerId, realModel = 'glm-4.6') {
  return { id, enabled: true, clientModel, providerId, realModel };
}

test('首次运行：文件不存在时返回默认配置且标记 firstRun', (t) => {
  const { manager } = makeManager(t);
  const result = manager.load();
  assert.equal(result.firstRun, true);
  assert.equal(result.recovered, false);
  assert.equal(result.config.settings.port, DEFAULT_PORT);
  assert.deepEqual(result.config.protocols.map((p) => p.id), ['anthropic', 'openai']);
});

test('保存后重新读取内容一致', (t) => {
  const { manager } = makeManager(t);
  const config = createDefaultConfig();
  config.protocols[0].providers.push(provider('p_zhipu'));
  config.protocols[0].mappings.push(mapping('m1', 'claude-*', 'p_zhipu'));
  manager.save(config);
  const reloaded = manager.load();
  assert.equal(reloaded.config.protocols[0].providers[0].id, 'p_zhipu');
  assert.equal(reloaded.config.protocols[0].mappings[0].clientModel, 'claude-*');
});

test('保存是原子的：不留 .tmp 残留且文件权限为 600', (t) => {
  const { dir, manager } = makeManager(t);
  manager.save(createDefaultConfig());
  assert.deepEqual(fs.readdirSync(dir), ['app.json']);
  assert.equal(fs.statSync(path.join(dir, 'app.json')).mode & 0o777, 0o600);
});

test('配置不是合法 JSON 时备份 .bak 并回落默认配置', (t) => {
  const { dir, manager } = makeManager(t);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'app.json'), '{ 这不是 json');
  const result = manager.load();
  assert.equal(result.recovered, true);
  assert.ok(result.backupPath.endsWith('app.json.bak'));
  assert.equal(fs.readFileSync(result.backupPath, 'utf8'), '{ 这不是 json');
  assert.equal(result.config.settings.port, DEFAULT_PORT);
});

test('配置根节点不是对象时判定为损坏', (t) => {
  const { dir, manager } = makeManager(t);
  fs.writeFileSync(path.join(dir, 'app.json'), '[1,2,3]');
  const result = manager.load();
  assert.equal(result.recovered, true);
  assert.equal(validateConfig([]).ok, false);
});

test('校验：端口非法回落默认并记 issue', () => {
  const result = validateConfig({ settings: { port: 'abc' }, protocols: [] });
  assert.equal(result.ok, true);
  assert.equal(result.config.settings.port, DEFAULT_PORT);
  assert.ok(result.issues.some((issue) => issue.includes('port')));
});

test('校验：未知协议 id 被丢弃并记 issue', () => {
  const result = validateConfig({
    settings: { port: 19000 },
    protocols: [
      { id: 'gemini', name: 'Gemini', enabled: true, providers: [], mappings: [] },
      { id: 'anthropic', name: 'Anthropic', enabled: true, providers: [], mappings: [] }
    ]
  });
  assert.deepEqual(result.config.protocols.map((p) => p.id), ['anthropic', 'openai']);
  assert.ok(result.issues.some((issue) => issue.includes('gemini')));
});

test('校验：缺 baseUrl 的供应商被丢弃，缺 providerId 的映射被丢弃', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [provider('ok'), { id: 'bad', name: 'bad', token: 't', models: [] }],
      mappings: [mapping('m1', 'a', 'ok'), { id: 'm2', clientModel: 'b', realModel: 'x' }]
    }]
  });
  const protocol = result.config.protocols[0];
  assert.deepEqual(protocol.providers.map((p) => p.id), ['ok']);
  assert.deepEqual(protocol.mappings.map((m) => m.id), ['m1']);
});

test('校验：映射指向不存在的供应商时被丢弃', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [provider('p1')], mappings: [mapping('m1', 'a', 'ghost')]
    }]
  });
  assert.equal(result.config.protocols[0].mappings.length, 0);
  assert.ok(result.issues.some((issue) => issue.includes('ghost')));
});

test('校验：配置里残留的 activeProviderId 不再保留', () => {
  // 走哪个供应商只由映射表决定，没有第二份「当前启用」状态可以跟它打架
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      activeProviderId: 'p1', providers: [provider('p1')], mappings: [mapping('m9', '*', 'p1')]
    }]
  });
  assert.equal('activeProviderId' in result.config.protocols[0], false);
  assert.equal(result.config.protocols[0].mappings[0].clientModel, '*');
});

