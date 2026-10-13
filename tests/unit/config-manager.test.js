// 配置管理单测：默认配置、原子写、损坏回落、校验归一化（含模型配置条目）
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const {
  createConfigManager,
  createDefaultConfig,
  createClientToken,
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

test('校验：模型条目新旧两种写法都认，旧字符串升级成配置条目', () => {
  const result = validateConfig({
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [provider('p1', [
        'glm-5.3',
        { name: 'gpt-5.5', contextWindow: 400000, maxContextWindow: 1024000, inputModalities: ['text', 'image'], reasoning: true }
      ])],
      mappings: []
    }]
  });
  const models = result.config.protocols[1].providers[0].models;
  assert.deepEqual(models, [
    // 旧格式只带得回名字，窗口与能力留空，界面上就是「没配」
    { name: 'glm-5.3' },
    { name: 'gpt-5.5', contextWindow: 400000, maxContextWindow: 1024000, inputModalities: ['text', 'image'], reasoning: true }
  ]);
});

test('校验：模型的非法字段只丢字段，缺名与重名的条目才整个丢掉', () => {
  const result = validateConfig({
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [provider('p1', [
        // 窗口非法、模态里混了白名单外的值、reasoning 不是布尔：字段级丢弃
        { name: 'ok', contextWindow: 'abc', maxContextWindow: 0, inputModalities: ['text', 'video'], reasoning: 'yes' },
        // 宽松强转也要挡住：true 与数组都不是合法的窗口值
        { name: 'typed', contextWindow: true, maxContextWindow: [400000] },
        // null 与窗口字段同口径，当「没配」处理，不弹警告
        { name: 'nulled', inputModalities: null, reasoning: null },
        { name: '   ' },
        null,
        // 重名：丢后者，留下先出现的那个，用窗口值把「留的是哪条」锁住
        { name: 'dup', contextWindow: 111 },
        { name: 'dup', contextWindow: 222 }
      ])],
      mappings: []
    }]
  });
  const models = result.config.protocols[0].providers[0].models;
  assert.deepEqual(models, [
    { name: 'ok', inputModalities: ['text'] },
    { name: 'typed' },
    { name: 'nulled' },
    { name: 'dup', contextWindow: 111 }
  ]);
  // null 当留空：不该为它记 issue
  assert.ok(!result.issues.some((issue) => issue.includes('nulled')));
  assert.ok(result.issues.some((issue) => issue.includes('contextWindow')));
  assert.ok(result.issues.some((issue) => issue.includes('maxContextWindow')));
  assert.ok(result.issues.some((issue) => issue.includes('inputModalities')));
  assert.ok(result.issues.some((issue) => issue.includes('reasoning')));
  assert.ok(result.issues.some((issue) => issue.includes('缺少 name')));
  assert.ok(result.issues.some((issue) => issue.includes('非法模型条目')));
  assert.ok(result.issues.some((issue) => issue.includes('重复模型 dup')));
});

test('校验：窗口留空不写回字段，重复模态去重', () => {
  const result = validateConfig({
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [provider('p1', [{ name: 'a', contextWindow: '', maxContextWindow: null, inputModalities: ['text', 'text', 'image'] }])],
      mappings: []
    }]
  });
  const model = result.config.protocols[1].providers[0].models[0];
  assert.deepEqual(model, { name: 'a', inputModalities: ['text', 'image'] });
});

test('校验：clientDirs 只收 id/kind/path 齐全且路径为绝对路径的条目，重复路径丢后者', () => {
  const good = { id: 'd1', kind: 'claude', path: '/Users/x/.claude-x' };
  const result = validateConfig({
    settings: {
      clientDirs: [
        good,
        // 相对路径：扫描按原样去找会随工作目录漂移，直接丢
        { id: 'd2', kind: 'codex', path: 'relative/.codex' },
        // kind 白名单外
        { id: 'd3', kind: 'gemini', path: '/Users/x/.gemini' },
        // 缺 id
        { kind: 'claude', path: '/Users/x/a' },
        // 与 good 同路径：重复
        { id: 'd4', kind: 'claude', path: '/Users/x/.claude-x' },
        null
      ]
    }
  });
  assert.deepEqual(result.config.settings.clientDirs, [good]);
  assert.ok(result.issues.some((issue) => issue.includes('绝对路径')));
  assert.ok(result.issues.some((issue) => issue.includes('kind 非法')));
  assert.ok(result.issues.some((issue) => issue.includes('缺少 id')));
  assert.ok(result.issues.some((issue) => issue.includes('重复目录')));
});

test('校验：clientDirs 不是数组重置为空，缺省也为空', () => {
  const broken = validateConfig({ settings: { clientDirs: 'nope' } });
  assert.deepEqual(broken.config.settings.clientDirs, []);
  assert.ok(broken.issues.some((issue) => issue.includes('clientDirs')));

  const absent = validateConfig({ protocols: [] });
  assert.deepEqual(absent.config.settings.clientDirs, []);
});

test('保存后 clientDirs 完整往返', (t) => {
  const { manager } = makeManager(t);
  const config = createDefaultConfig();
  config.settings.clientDirs = [{ id: 'd9', kind: 'codex', path: '/Users/x/codex-config' }];
  manager.save(config);
  const reloaded = manager.load();
  assert.deepEqual(reloaded.config.settings.clientDirs, [{ id: 'd9', kind: 'codex', path: '/Users/x/codex-config' }]);
});

test('校验：clientToken 只留字符串，非法值清成空串', () => {
  const kept = validateConfig({ settings: { clientToken: '  t_abc  ' } });
  assert.equal(kept.config.settings.clientToken, 't_abc');

  // 生成放在主进程启动阶段做（校验是纯函数，这里生成就没法单测了）
  for (const bad of [undefined, null, 42, { a: 1 }]) {
    assert.equal(validateConfig({ settings: { clientToken: bad } }).config.settings.clientToken, '');
  }
  assert.equal(validateConfig({}).config.settings.clientToken, '');
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

test('createClientToken：sk- 前缀 + 32 位十六进制，且每次生成都不同', () => {
  const token = createClientToken();
  assert.match(token, /^sk-[0-9a-f]{32}$/);
  assert.notEqual(token, createClientToken());
});

test('默认配置：日志与用量开关都是开启', () => {
  const config = createDefaultConfig();
  assert.equal(config.settings.logsEnabled, true);
  assert.equal(config.settings.usageEnabled, true);
});

test('校验：日志与用量开关缺省即开启，显式 false 才关', () => {
  // 老配置里没有这两个键，读回来必须还是开启，行为不回退
  const absent = validateConfig({ protocols: [] });
  assert.equal(absent.config.settings.logsEnabled, true);
  assert.equal(absent.config.settings.usageEnabled, true);

  const off = validateConfig({ settings: { logsEnabled: false, usageEnabled: false } });
  assert.equal(off.config.settings.logsEnabled, false);
  assert.equal(off.config.settings.usageEnabled, false);

  // 手改配置塞进非布尔值：按「不等于 false」处理，仍是开启
  const weird = validateConfig({ settings: { logsEnabled: 'no', usageEnabled: 0 } });
  assert.equal(weird.config.settings.logsEnabled, true);
  assert.equal(weird.config.settings.usageEnabled, true);
});

