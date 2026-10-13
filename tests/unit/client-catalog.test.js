// codex 模型清单单测：期望条目的推导、清单检查、定点改写，以及检查/写入两条链路的联动。
// 底线与 config.toml 那套一样：只动归我们管的条目与字段，别的供应商的内容一个字节不碰
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { expectedCatalogEntries, inspectCatalog, rewriteCatalog, catalogFilePath } = require('../../electron/services/clients/catalog');
const { inspectClientConfig } = require('../../electron/services/clients/check');
const { applyClientConfig, rewriteCodexConfig, syncCodexCatalogs } = require('../../electron/services/clients/apply');

const CONTEXT = { port: 18888, token: 't_abc123' };

// 一条合格的清单条目：字段齐全，能力值由调用方按需覆盖
function makeEntry(slug, overrides = {}) {
  return {
    slug,
    display_name: slug,
    description: slug,
    base_instructions: 'You are Codex.',
    context_window: 1024000,
    max_context_window: 1024000,
    input_modalities: ['text', 'image'],
    default_reasoning_level: 'high',
    supported_reasoning_levels: [
      { effort: 'none', description: 'Disable thinking' },
      { effort: 'low', description: 'Fast responses with lighter reasoning' },
      { effort: 'high', description: 'Enabled high reasoning' },
      { effort: 'max', description: 'Maximum reasoning depth for the hardest problems' }
    ],
    shell_type: 'shell_command',
    visibility: 'list',
    supported_in_api: true,
    priority: 1000,
    support_verbosity: false,
    truncation_policy: { mode: 'bytes', limit: 10000 },
    experimental_supported_tools: [],
    ...overrides
  };
}

// 期望条目：名字是映射的 clientModel，能力取映射指向的真实模型
function makeWant(name, overrides = {}) {
  return { name, contextWindow: 1024000, maxContextWindow: 1024000, inputModalities: ['text', 'image'], reasoning: true, ...overrides };
}

function catalogText(...entries) {
  return `${JSON.stringify({ models: entries }, null, 2)}\n`;
}

// ============================================================================ 期望条目推导

test('期望条目：名字用 clientModel，能力取映射指向的真实模型', () => {
  const protocol = {
    providers: [
      {
        id: 'p1',
        enabled: true,
        models: [
          { name: 'real-a', contextWindow: 1024000, maxContextWindow: 2048000, inputModalities: ['text', 'image'], reasoning: true },
          { name: 'real-b' }
        ]
      },
      { id: 'p2', enabled: false, models: [{ name: 'x' }] }
    ],
    mappings: [
      { clientModel: 'client-a', providerId: 'p1', realModel: 'real-a', enabled: true },
      { clientModel: 'client-b', providerId: 'p1', realModel: 'real-b', enabled: false },
      { clientModel: '*', providerId: 'p1', realModel: 'real-a', enabled: true },
      { clientModel: 'client-x', providerId: 'p2', realModel: 'x', enabled: true }
    ]
  };
  // 停用的映射、通配/兜底映射、停用供应商的映射都不算模型
  assert.deepEqual(expectedCatalogEntries(protocol), [
    { name: 'client-a', contextWindow: 1024000, maxContextWindow: 2048000, inputModalities: ['text', 'image'], reasoning: true }
  ]);
});

test('期望条目：真实模型没配的能力字段是 undefined（比对该跳过、写入不落笔）', () => {
  const entries = expectedCatalogEntries({
    providers: [{ id: 'p1', enabled: true, models: [{ name: 'real-a' }] }],
    mappings: [{ clientModel: 'client-a', providerId: 'p1', realModel: 'real-a', enabled: true }]
  });
  assert.deepEqual(entries, [{ name: 'client-a', contextWindow: undefined, maxContextWindow: undefined, inputModalities: undefined, reasoning: false }]);
});

test('期望条目：手编配置绕过界面的重名映射只认第一条，协议停用时一条都不算', () => {
  const protocol = {
    providers: [{ id: 'p1', enabled: true, models: [{ name: 'real-a', contextWindow: 111 }, { name: 'real-b', contextWindow: 222 }] }],
    mappings: [
      { clientModel: 'client-a', providerId: 'p1', realModel: 'real-a', enabled: true },
      { clientModel: 'client-a', providerId: 'p1', realModel: 'real-b', enabled: true }
    ]
  };
  assert.deepEqual(expectedCatalogEntries(protocol).map((entry) => entry.contextWindow), [111]);
  assert.deepEqual(expectedCatalogEntries({ ...protocol, enabled: false }), []);
});

// ============================================================================ 检查

test('检查：条目与配置一致时 ok', () => {
  const result = inspectCatalog(catalogText(makeEntry('client-a')), [makeWant('client-a')]);
  assert.equal(result.status, 'ok');
  assert.deepEqual(result.problems, []);
});

test('检查：缺条目、窗口/模态/推理不一致都各有各的提示', () => {
  const result = inspectCatalog(
    catalogText(
      makeEntry('client-a', { context_window: 999, max_context_window: 999, input_modalities: ['text'], supported_reasoning_levels: [] }),
      makeEntry('other-provider', { context_window: 1 })
    ),
    [makeWant('client-a'), makeWant('client-b')]
  );
  assert.equal(result.status, 'mismatch');
  assert.ok(result.problems.some((item) => item.includes('没有 client-b')));
  assert.ok(result.problems.some((item) => item.includes('上下文窗口是 999') && item.includes('1024000')));
  assert.ok(result.problems.some((item) => item.includes('最大上下文窗口是 999')));
  assert.ok(result.problems.some((item) => item.includes('输入模态是 text')));
  assert.ok(result.problems.some((item) => item.includes('没有推理档位')));
  // 别的供应商的条目不比能力
  assert.ok(!result.problems.some((item) => item.includes('other-provider')));
});

test('检查：CCProxy 没勾推理而清单配了档位，也要报', () => {
  const result = inspectCatalog(catalogText(makeEntry('client-a')), [makeWant('client-a', { reasoning: false })]);
  assert.equal(result.status, 'mismatch');
  assert.ok(result.problems.some((item) => item.includes('配了推理档位')));
});

test('检查：CCProxy 没配的字段不挑清单的错', () => {
  const result = inspectCatalog(catalogText(makeEntry('client-a')), [makeWant('client-a', { contextWindow: undefined, maxContextWindow: undefined, inputModalities: undefined })]);
  assert.equal(result.status, 'ok');
});

test('检查：坏 JSON 读不出来；slug 重复、条目缺必需字段都有提示', () => {
  assert.equal(inspectCatalog('{ 坏的', [makeWant('client-a')]).status, 'unreadable');

  const duplicated = inspectCatalog(catalogText(makeEntry('client-a'), makeEntry('client-a')), [makeWant('client-a')]);
  assert.ok(duplicated.problems.some((item) => item.includes('出现了 2 次')));

  const broken = inspectCatalog(catalogText({ slug: 'client-a', display_name: 'x' }), [makeWant('client-a')]);
  assert.ok(broken.problems.some((item) => item.includes('缺必需字段')));
  assert.ok(broken.problems.some((item) => item.includes('缺 base_instructions')));
});

test('检查：别人的条目缺必需键只提示不归我们管', () => {
  const result = inspectCatalog(catalogText(makeEntry('client-a'), { slug: 'foreign', display_name: 'x' }), [makeWant('client-a')]);
  assert.equal(result.status, 'mismatch');
  assert.ok(result.problems.some((item) => item.includes('foreign') && item.includes('不归 CCProxy 管')));
});

// ============================================================================ 改写

test('改写：已有条目只改能力字段，别的条目与全文一个字节不动', () => {
  const before = catalogText(makeEntry('client-a', { context_window: 999 }), makeEntry('foreign'));
  const { text, changed } = rewriteCatalog(before, [makeWant('client-a')]);
  assert.equal(changed, true);
  const after = JSON.parse(text);
  assert.equal(after.models[0].context_window, 1024000);
  // 别的条目原样
  assert.deepEqual(after.models[1], makeEntry('foreign'));
  // 键序与缩进保留（定点换值不重排版）
  assert.ok(text.startsWith('{\n  "models": [\n    {\n      "slug": "client-a"'));
});

test('改写：缺的条目按模板补，抄模板的字段、换掉身份与能力', () => {
  const before = catalogText(makeEntry('client-a', { priority: 1001, base_instructions: '模板的指令' }));
  const { text } = rewriteCatalog(before, [makeWant('client-a'), makeWant('client-b', { reasoning: false })]);
  const after = JSON.parse(text);
  assert.equal(after.models.length, 2);
  const added = after.models[1];
  assert.equal(added.slug, 'client-b');
  assert.equal(added.display_name, 'client-b');
  assert.equal(added.priority, 1001); // 沿用模板
  assert.equal(added.base_instructions, '模板的指令'); // 沿用模板
  assert.deepEqual(added.supported_reasoning_levels, []); // 没勾推理就没有档位
});

test('改写：清单是空的就从零建条目，必需字段齐全', () => {
  const { text } = rewriteCatalog('{\n  "models": []\n}\n', [makeWant('client-a')]);
  const after = JSON.parse(text);
  assert.equal(after.models.length, 1);
  assert.equal(after.models[0].slug, 'client-a');
  for (const key of ['slug', 'display_name', 'supported_reasoning_levels', 'shell_type', 'visibility', 'supported_in_api', 'priority', 'support_verbosity', 'truncation_policy', 'experimental_supported_tools', 'base_instructions']) {
    assert.ok(key in after.models[0], `缺必需字段 ${key}`);
  }
});

test('改写：已一致的清单不落笔（changed=false），坏 JSON 抛错', () => {
  const text = catalogText(makeEntry('client-a'));
  assert.deepEqual(rewriteCatalog(text, [makeWant('client-a')]), { text, changed: false });
  assert.throws(() => rewriteCatalog('{ 坏的', [makeWant('client-a')]), /为免写坏已放弃/);
});

test('改写：与我们重名的多余条目删掉，修完检查能收敛到 ok', () => {
  const before = catalogText(makeEntry('client-a', { context_window: 999 }), makeEntry('foreign'), makeEntry('client-a', { context_window: 999 }));
  const { text } = rewriteCatalog(before, [makeWant('client-a')]);
  const after = JSON.parse(text);
  assert.equal(after.models.filter((entry) => entry.slug === 'client-a').length, 1);
  assert.equal(after.models[0].context_window, 1024000);
  // 中间那个别人的条目和两个重名之间的逗号一起被带走，剩下的原样
  assert.equal(after.models.length, 2);
  assert.equal(after.models[1].slug, 'foreign');
  assert.equal(inspectCatalog(text, [makeWant('client-a')]).status, 'ok');
});

// ============================================================================ 检查/写入链路

function makeDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-catalog-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function writeCodexConfig(dir, text) {
  fs.writeFileSync(path.join(dir, 'config.toml'), text);
}

// 一份指向 CCProxy 的合法 config.toml（不带 model_catalog_json）
const BASE_TOML = [
  'model_provider = "ccproxy"',
  '',
  '[model_providers.ccproxy]',
  'name = "ccproxy"',
  'base_url = "http://127.0.0.1:18888/v1"',
  'wire_api = "responses"',
  'requires_openai_auth = false',
  `experimental_bearer_token = "${CONTEXT.token}"`,
  ''
].join('\n');

test('链路：目录检查带上清单口径——指向键缺失、清单缺失都要报，应用后变 ok', (t) => {
  const dir = makeDir(t);
  writeCodexConfig(dir, BASE_TOML);
  const context = { ...CONTEXT, catalog: [makeWant('client-a')] };

  const before = inspectClientConfig(dir, 'codex', context);
  assert.equal(before.status, 'mismatch');
  assert.ok(before.problems.some((item) => item.includes('model_catalog_json')));
  assert.ok(before.problems.some((item) => item.includes('models.json')));

  const result = applyClientConfig(dir, 'codex', context);
  // config.toml 与 models.json 各写一份
  assert.deepEqual(result.map((item) => path.basename(item.file)).sort(), ['config.toml', 'models.json']);

  const after = inspectClientConfig(dir, 'codex', context);
  assert.equal(after.status, 'ok');
  const toml = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  assert.ok(toml.includes(`model_catalog_json = "${catalogFilePath(dir)}"`));
});

test('链路：指向别处的 model_catalog_json 只换值，用户行内的注释保留', () => {
  // 顶层键必须写在第一个节之前，这是合法的 TOML 位置
  const source = ['model_provider = "ccproxy"', 'model_catalog_json = "/tmp/别人的.json" # 我自己的', '', '[model_providers.ccproxy]', 'name = "ccproxy"'].join('\n');
  const text = rewriteCodexConfig(source, { ...CONTEXT, catalogPath: '/tmp/x/models.json' });
  assert.match(text, /^model_catalog_json = "\/tmp\/x\/models\.json" # 我自己的$/m);
  assert.equal(text.match(/^model_catalog_json = /gm).length, 1);
});

test('链路：模型清单没变化时不落盘（不覆盖备份、不动文件）', (t) => {
  const dir = makeDir(t);
  writeCodexConfig(dir, BASE_TOML);
  const catalog = catalogText(makeEntry('client-a'));
  fs.writeFileSync(path.join(dir, 'models.json'), catalog);
  const context = { ...CONTEXT, catalog: [makeWant('client-a')] };

  const result = applyClientConfig(dir, 'codex', context);
  const modelsWrite = result.find((item) => item.file.endsWith('models.json'));
  assert.equal(modelsWrite.backupPath, null); // 核对了但没落笔
  assert.equal(fs.readFileSync(path.join(dir, 'models.json'), 'utf8'), catalog);
  assert.deepEqual(fs.readdirSync(dir).sort(), ['config.toml', 'config.toml.ccproxy-bak', 'models.json']);
});

test('链路：没有归我们管的模型时，清单与指向键都不碰', (t) => {
  const dir = makeDir(t);
  writeCodexConfig(dir, BASE_TOML);
  const context = { ...CONTEXT, catalog: [] };

  assert.equal(inspectClientConfig(dir, 'codex', context).status, 'ok');
  applyClientConfig(dir, 'codex', context);
  assert.deepEqual(fs.readdirSync(dir).sort(), ['config.toml']);
  assert.ok(!fs.readFileSync(path.join(dir, 'config.toml'), 'utf8').includes('model_catalog_json'));
});

test('链路：models.json 坏掉时整个目录报 unreadable，应用放弃写入不覆盖用户那份', (t) => {
  const dir = makeDir(t);
  writeCodexConfig(dir, BASE_TOML);
  fs.writeFileSync(path.join(dir, 'models.json'), '{ 坏的');
  const context = { ...CONTEXT, catalog: [makeWant('client-a')] };

  assert.equal(inspectClientConfig(dir, 'codex', context).status, 'unreadable');
  assert.throws(() => applyClientConfig(dir, 'codex', context), /models\.json/);
  assert.equal(fs.readFileSync(path.join(dir, 'models.json'), 'utf8'), '{ 坏的');
});

// ============================================================================ 自动同步

// 一个「已归我们管」的 codex 目录（config.toml 指向 CCProxy 且指向键已配好），
// 清单内容停在旧配置上
function makeManagedDir(t, catalog) {
  const dir = makeDir(t);
  // 指向键是顶层键，必须写在第一个节之前
  const toml = BASE_TOML.replace('model_provider = "ccproxy"\n', `model_provider = "ccproxy"\nmodel_catalog_json = "${catalogFilePath(dir)}"\n`);
  writeCodexConfig(dir, toml);
  fs.writeFileSync(path.join(dir, 'models.json'), catalog);
  return dir;
}

test('自动同步：已托管的目录清单刷成最新，没变化的目录不动、别人家的不碰', (t) => {
  const fresh = makeManagedDir(t, catalogText(makeEntry('client-a')));
  const stale = makeManagedDir(t, catalogText(makeEntry('client-a', { context_window: 999 })));
  const context = { ...CONTEXT, catalog: [makeWant('client-a')] };

  const { applied, errors } = syncCodexCatalogs(
    [{ kind: 'claude', path: makeDir(t) }, { kind: 'codex', path: fresh }, { kind: 'codex', path: stale }],
    context
  );
  assert.deepEqual(errors, []);
  // 没变化的目录核对了但不落盘，只有过期的那个写了 models.json
  assert.deepEqual(applied.map((item) => item.file), [path.join(stale, 'models.json')]);
  assert.equal(JSON.parse(fs.readFileSync(path.join(stale, 'models.json'), 'utf8')).models[0].context_window, 1024000);
  // config.toml 没变化也不落盘
  assert.ok(!fs.existsSync(path.join(stale, 'config.toml.ccproxy-bak')));
});

test('自动同步：没归我们管的目录（地址或 token 不对）不替用户做主', (t) => {
  const dir = makeDir(t);
  writeCodexConfig(dir, BASE_TOML.replace('t_abc123', '别人的-token'));
  const context = { ...CONTEXT, catalog: [makeWant('client-a')] };

  const { applied, errors } = syncCodexCatalogs([{ kind: 'codex', path: dir }], context);
  assert.deepEqual(applied, []);
  assert.deepEqual(errors, []);
  assert.deepEqual(fs.readdirSync(dir), ['config.toml']); // 连清单都没给它建
});

test('自动同步：一个目录坏掉不影响别的目录，失败按目录收集', (t) => {
  const broken = makeManagedDir(t, '{ 坏的');
  const stale = makeManagedDir(t, catalogText(makeEntry('client-a', { context_window: 999 })));
  const context = { ...CONTEXT, catalog: [makeWant('client-a')] };

  const { applied, errors } = syncCodexCatalogs([{ kind: 'codex', path: broken }, { kind: 'codex', path: stale }], context);
  assert.equal(errors.length, 1);
  assert.equal(errors[0].path, broken);
  assert.ok(errors[0].message.includes('models.json'));
  assert.deepEqual(applied.map((item) => item.file), [path.join(stale, 'models.json')]);
  assert.equal(JSON.parse(fs.readFileSync(path.join(stale, 'models.json'), 'utf8')).models[0].context_window, 1024000);
  assert.equal(fs.readFileSync(path.join(broken, 'models.json'), 'utf8'), '{ 坏的'); // 坏的原样
});
