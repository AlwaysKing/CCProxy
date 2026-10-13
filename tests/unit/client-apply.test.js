// 写入客户端配置单测：claude 的 settings.json、codex 的 config.toml 改写，
// 以及「只动该动的、其余原样」这条底线（用户文件里别的东西一个都不能少）
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { applyClientConfig, rewriteCodexConfig } = require('../../electron/services/clients/apply');
const { inspectClientConfig } = require('../../electron/services/clients/check');

const CONTEXT = { port: 18888, token: 't_abc123' };

function makeDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-apply-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function writeFile(dir, name, content) {
  const filePath = path.join(dir, name);
  fs.writeFileSync(filePath, content);
  return filePath;
}

test('claude：写 env 的两个键，其余内容一个不动', (t) => {
  const dir = makeDir(t);
  const original = {
    permissions: { allow: ['Bash(ls:*)'] },
    model: 'claude-sonnet-5',
    enabledPlugins: { 'x@y': true },
    env: {
      ANTHROPIC_BASE_URL: 'http://192.168.60.2:9005',
      ANTHROPIC_AUTH_TOKEN: 'sk-old',
      ANTHROPIC_MODEL: 'claude-sonnet-5',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'glm-5.3'
    }
  };
  writeFile(dir, 'settings.json', JSON.stringify(original, null, 2));

  const result = applyClientConfig(dir, 'claude', CONTEXT);
  const written = JSON.parse(fs.readFileSync(result[0].file, 'utf8'));

  assert.equal(written.env.ANTHROPIC_BASE_URL, 'http://127.0.0.1:18888');
  assert.equal(written.env.ANTHROPIC_AUTH_TOKEN, 't_abc123');
  // 模型名不碰（模型映射是另一件事），权限与插件原样
  assert.equal(written.env.ANTHROPIC_MODEL, 'claude-sonnet-5');
  assert.equal(written.env.ANTHROPIC_DEFAULT_OPUS_MODEL, 'glm-5.3');
  assert.deepEqual(written.permissions, original.permissions);
  assert.deepEqual(written.enabledPlugins, original.enabledPlugins);
  assert.equal(written.model, 'claude-sonnet-5');
});

test('claude：写入后检查变 ok，且留了一份备份', (t) => {
  const dir = makeDir(t);
  const filePath = writeFile(dir, 'settings.json', JSON.stringify({ env: { ANTHROPIC_BASE_URL: 'http://192.168.60.2:9005' } }));

  const result = applyClientConfig(dir, 'claude', CONTEXT);
  assert.equal(result[0].backupPath, `${filePath}.ccproxy-bak`);
  assert.equal(fs.readFileSync(result[0].backupPath, 'utf8'), JSON.stringify({ env: { ANTHROPIC_BASE_URL: 'http://192.168.60.2:9005' } }));
  assert.equal(inspectClientConfig(dir, 'claude', CONTEXT).status, 'ok');
});

test('claude：只换那两个值的字节区间，缩进/键序/转义/行尾/末尾换行一个都不动', (t) => {
  const dir = makeDir(t);
  // 4 空格缩进、CRLF 行尾、\\u 转义、末尾没有换行——全是「不该被我们改掉」的字节
  const before = [
    '{',
    '    "permissions": {',
    '        "allow": [ "Bash(ls:*)" ]',
    '    },',
    '    "env": {',
    '        "ANTHROPIC_AUTH_TOKEN": "sk-old",',
    '        "ANTHROPIC_MODEL": "claude-\\u00e9",',
    '        "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005"',
    '    }',
    '}'
  ].join('\r\n');
  writeFile(dir, 'settings.json', before);

  applyClientConfig(dir, 'claude', CONTEXT);
  const after = fs.readFileSync(path.join(dir, 'settings.json'), 'utf8');
  assert.equal(
    after,
    before
      .replace('"http://192.168.60.2:9005"', '"http://127.0.0.1:18888"')
      .replace('"sk-old"', `"${CONTEXT.token}"`)
  );
  // 转义没被解码、行尾没被换、末尾也没被补换行
  assert.ok(after.includes('"claude-\\u00e9"'));
  assert.ok(after.includes('\r\n'));
  assert.ok(!after.endsWith('\n'));
});

test('claude：只有一个值不对就只改那一行', (t) => {
  const dir = makeDir(t);
  const before = '{\n  "env": {\n    "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005",\n    "ANTHROPIC_AUTH_TOKEN": "t_abc123"\n  }\n}\n';
  writeFile(dir, 'settings.json', before);

  applyClientConfig(dir, 'claude', CONTEXT);
  assert.equal(
    fs.readFileSync(path.join(dir, 'settings.json'), 'utf8'),
    before.replace('"http://192.168.60.2:9005"', '"http://127.0.0.1:18888"')
  );
});

test('claude：要写的都在了就不落盘（不覆盖备份、不动文件）', (t) => {
  const dir = makeDir(t);
  const before = JSON.stringify({ env: { ANTHROPIC_BASE_URL: 'http://127.0.0.1:18888', ANTHROPIC_AUTH_TOKEN: 't_abc123' } }, null, 4);
  const filePath = writeFile(dir, 'settings.json', before);

  const result = applyClientConfig(dir, 'claude', CONTEXT);
  assert.equal(result[0].backupPath, null);
  assert.deepEqual(fs.readdirSync(dir), ['settings.json']);
  assert.equal(fs.readFileSync(filePath, 'utf8'), before);
});

test('claude：文件不存在时新建；没有 env 段时补一个', (t) => {
  const empty = makeDir(t);
  applyClientConfig(empty, 'claude', CONTEXT);
  assert.equal(inspectClientConfig(empty, 'claude', CONTEXT).status, 'ok');

  const noEnv = makeDir(t);
  writeFile(noEnv, 'settings.json', JSON.stringify({ model: 'x' }));
  applyClientConfig(noEnv, 'claude', CONTEXT);
  const written = JSON.parse(fs.readFileSync(path.join(noEnv, 'settings.json'), 'utf8'));
  assert.equal(written.model, 'x');
  assert.equal(written.env.ANTHROPIC_BASE_URL, 'http://127.0.0.1:18888');
});

test('claude：settings.json 坏掉时放弃写入，不覆盖用户那份', (t) => {
  const dir = makeDir(t);
  const filePath = writeFile(dir, 'settings.json', '{ 这不是 json');
  assert.throws(() => applyClientConfig(dir, 'claude', CONTEXT), /为免写坏已放弃/);
  // 原文件与备份都不该被创建/改动
  assert.equal(fs.readFileSync(filePath, 'utf8'), '{ 这不是 json');
  assert.deepEqual(fs.readdirSync(dir), ['settings.json']);
});

test('claude：env 是个空对象也能把两个键补进去', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'settings.json', '{\n  "env": {}\n}\n');

  applyClientConfig(dir, 'claude', CONTEXT);
  const written = JSON.parse(fs.readFileSync(path.join(dir, 'settings.json'), 'utf8'));
  assert.equal(written.env.ANTHROPIC_BASE_URL, 'http://127.0.0.1:18888');
  assert.equal(written.env.ANTHROPIC_AUTH_TOKEN, 't_abc123');
  assert.equal(inspectClientConfig(dir, 'claude', CONTEXT).status, 'ok');
});

test('claude：原子写不留临时文件，权限沿用原文件', (t) => {
  const dir = makeDir(t);
  const filePath = writeFile(dir, 'settings.json', '{}');
  fs.chmodSync(filePath, 0o640);
  applyClientConfig(dir, 'claude', CONTEXT);
  assert.deepEqual(fs.readdirSync(dir).sort(), ['settings.json', 'settings.json.ccproxy-bak']);
  assert.equal(fs.statSync(filePath).mode & 0o777, 0o640);
});

test('claude：权限完整沿用（新建文件时 umask 会削掉 0666 的组/其他写位）', (t) => {
  const dir = makeDir(t);
  const filePath = writeFile(dir, 'settings.json', '{}');
  fs.chmodSync(filePath, 0o666);
  applyClientConfig(dir, 'claude', CONTEXT);
  assert.equal(fs.statSync(filePath).mode & 0o777, 0o666);
});

test('claude：settings.json 是软链时写真实文件，链接本身留着', (t) => {
  const dir = makeDir(t);
  const realDir = makeDir(t);
  // dotfiles 仓库那类用法：~/.claude/settings.json 软链到别处
  const realPath = writeFile(realDir, 'settings.json', JSON.stringify({ env: { ANTHROPIC_BASE_URL: 'http://192.168.60.2:9005' } }));
  const linkPath = path.join(dir, 'settings.json');
  fs.symlinkSync(realPath, linkPath);

  applyClientConfig(dir, 'claude', CONTEXT);
  assert.equal(fs.lstatSync(linkPath).isSymbolicLink(), true);
  assert.equal(JSON.parse(fs.readFileSync(realPath, 'utf8')).env.ANTHROPIC_BASE_URL, 'http://127.0.0.1:18888');
  // 备份落在真实文件旁边，链接目录里不留东西
  assert.ok(fs.existsSync(`${realPath}.ccproxy-bak`));
  assert.deepEqual(fs.readdirSync(dir), ['settings.json']);
  assert.equal(inspectClientConfig(dir, 'claude', CONTEXT).status, 'ok');
});

test('codex：顶层 model_provider 改指向 ccproxy，我们那个块写全', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'config.toml', [
    'model_provider = "ccswitch"',
    'model = "gpt-5.6-sol"',
    'model_reasoning_effort = "medium"',
    '',
    '[model_providers.ccswitch]',
    'name = "ccswitch"',
    'base_url = "http://192.168.60.2:9005/v1"',
    'wire_api = "responses"',
    'experimental_bearer_token = "sk-old"',
    '',
    '[features]',
    'multi_agent = true'
  ].join('\n'));

  applyClientConfig(dir, 'codex', CONTEXT);
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');

  // 用户自己那个 provider 块原样保留
  assert.match(text, /\[model_providers\.ccswitch\]\nname = "ccswitch"\nbase_url = "http:\/\/192\.168\.60\.2:9005\/v1"/);
  assert.match(text, /\[features\]\nmulti_agent = true/);
  // 顶层只剩一个 model_provider，且指向我们
  assert.deepEqual(text.match(/^model_provider = .*$/gm), ['model_provider = "ccproxy"']);
  assert.match(text, /\[model_providers\.ccproxy\]\nname = "ccproxy"\nbase_url = "http:\/\/127\.0\.0\.1:18888\/v1"\nwire_api = "responses"\nrequires_openai_auth = false\nexperimental_bearer_token = "t_abc123"/);
  // model 不动：模型是用户自己在 CCProxy 里配映射的事
  assert.match(text, /^model = "gpt-5\.6-sol"$/m);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：已有 ccproxy 块就整块重写，不留新旧两套', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'config.toml', [
    'model_provider = "ccproxy"',
    '',
    '[model_providers.ccproxy]',
    '# 旧的说明',
    'name = "old"',
    'base_url = "http://192.168.60.2:9005/v1"',
    'experimental_bearer_token = "sk-old"',
    'wire_api = "responses"',
    '',
    '[features]',
    'multi_agent = true'
  ].join('\n'));

  applyClientConfig(dir, 'codex', CONTEXT);
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  assert.equal(text.match(/\[model_providers\.ccproxy\]/g).length, 1);
  assert.ok(!text.includes('sk-old'));
  assert.ok(!text.includes('旧的说明'));
  assert.ok(!text.includes('name = "old"'));
  assert.match(text, /\[features\]\nmulti_agent = true/);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：我们那个块之后的节名行带行尾注释也算边界，后面的内容一个不能少', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'config.toml', [
    'model_provider = "ccproxy"',
    '',
    '[model_providers.ccproxy]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"',
    '',
    '[model_providers.backup] # 备用供应商',
    '# 我自己的，别动',
    'base_url = "https://api.example.com/v1"',
    'experimental_bearer_token = "sk-secret"'
  ].join('\n'));

  applyClientConfig(dir, 'codex', CONTEXT);
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');

  // 带行尾注释的节名认不出来时，它连同下面整段会被当成「我们块的内容」删掉
  assert.match(text, /\[model_providers\.backup\] # 备用供应商\n# 我自己的，别动\nbase_url = "https:\/\/api\.example\.com\/v1"\nexperimental_bearer_token = "sk-secret"/);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：带注释或用引号包起来的同名块认得出来，不会追加出第二个同名表', (t) => {
  // 两个 [model_providers.ccproxy] 在 TOML 里是非法配置，codex 会直接读不动
  const headers = ['[model_providers.ccproxy] # CCProxy 写入的', '[model_providers."ccproxy"]'];
  for (const header of headers) {
    const dir = makeDir(t);
    writeFile(dir, 'config.toml', [
      'model_provider = "ccproxy"',
      '',
      header,
      'name = "old"',
      'base_url = "http://192.168.60.2:9005/v1"',
      'experimental_bearer_token = "sk-old"'
    ].join('\n'));

    applyClientConfig(dir, 'codex', CONTEXT);
    const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
    assert.equal(text.match(/^\[model_providers\.[^\]]*ccproxy[^\]]*\]/gm).length, 1, `${header} 之后出现了两个同名表`);
    assert.equal(text.match(/^experimental_bearer_token = /gm).length, 1, `${header} 之后新旧两套键值都留下了`);
    assert.ok(!text.includes('sk-old'));
    assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
  }
});

test('codex：三引号没闭合时放弃写入，连备份都不生成', (t) => {
  const dir = makeDir(t);
  const original = 'model_provider = "custom"\ndev = """\n这里没闭合\n';
  writeFile(dir, 'config.toml', original);

  // 这种文件已经不合法，扫描器会把后面整段当字符串内容，照写就把配置插进字符串里面了
  assert.throws(() => applyClientConfig(dir, 'codex', CONTEXT), /为免写坏已放弃/);
  assert.equal(fs.readFileSync(path.join(dir, 'config.toml'), 'utf8'), original);
  assert.deepEqual(fs.readdirSync(dir), ['config.toml']);
});

test('codex：只换 model_provider 那个值 token，行尾注释、缩进、CRLF、末尾换行都不动', (t) => {
  const dir = makeDir(t);
  const before = [
    '# 我的 codex 配置',
    'model_provider = "custom" # 走中转',
    'model = "gpt-5.6-sol"',
    '',
    '[model_providers.custom]',
    'base_url = "https://api.example.com/v1"'
  ].join('\r\n');
  writeFile(dir, 'config.toml', before);

  applyClientConfig(dir, 'codex', CONTEXT);
  const after = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  assert.ok(after.startsWith('# 我的 codex 配置\r\nmodel_provider = "ccproxy" # 走中转\r\nmodel = "gpt-5.6-sol"\r\n\r\n[model_providers.custom]\r\n'), '注释与 CRLF 被动了');
  // 用户的块之后空一行接我们的块，行尾跟文件一致
  assert.match(after, /\r\n\r\n\[model_providers\.ccproxy\]\r\nname = "ccproxy"\r\n/);
  assert.ok(after.endsWith(`experimental_bearer_token = "${CONTEXT.token}"\r\n`));
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：model_provider 已经指向我们（单引号写法）时那一行原样留着', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'config.toml', [
    "model_provider = 'ccproxy' # 写法是用户自己的",
    '',
    '[model_providers.ccproxy]',
    'name = "ccproxy"',
    `base_url = "http://127.0.0.1:18888/v1"`,
    'wire_api = "responses"',
    'requires_openai_auth = false',
    `experimental_bearer_token = "${CONTEXT.token}"`,
    ''
  ].join('\n'));

  const result = applyClientConfig(dir, 'codex', CONTEXT);
  assert.equal(result[0].backupPath, null); // 一个字节都没改，不落盘
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  assert.match(text, /^model_provider = 'ccproxy' # 写法是用户自己的$/m);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：原来那个 requires_openai_auth = true 的块不动，我们自己的块显式写 false', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'config.toml', [
    'model_provider = "custom"',
    '',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:15721/v1"',
    'requires_openai_auth = true'
  ].join('\n'));

  applyClientConfig(dir, 'codex', CONTEXT);
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  // 用户自己那个块原样留着（它只是变成没人引用的死配置，删它才是越界）
  assert.match(text, /\[model_providers\.custom\]\nbase_url = "http:\/\/127\.0\.0\.1:15721\/v1"\nrequires_openai_auth = true/);
  // 指向我们的那个块必须显式 false，否则 codex 会改用 auth.json 里的凭据，token 白填
  assert.match(text, /\[model_providers\.ccproxy\]\nname = "ccproxy"\nbase_url = "http:\/\/127\.0\.0\.1:18888\/v1"\nwire_api = "responses"\nrequires_openai_auth = false\n/);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：model_provider 指向不存在的块时，写入能把它修好', (t) => {
  const dir = makeDir(t);
  writeFile(dir, 'config.toml', 'model_provider = "ghost"\n\n[model_providers.other]\nbase_url = "http://x/v1"\n');
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'unreadable');

  applyClientConfig(dir, 'codex', CONTEXT);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：多行字符串里的空行与内容原样保留（那也算用户的内容）', (t) => {
  const dir = makeDir(t);
  const instructions = ['developer_instructions = """', '第一条', '', '', '第三条', '"""'].join('\n');
  writeFile(dir, 'config.toml', ['model_provider = "custom"', instructions, '', '[model_providers.custom]', 'base_url = "http://x/v1"'].join('\n'));

  applyClientConfig(dir, 'codex', CONTEXT);
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  assert.ok(text.includes('第一条\n\n\n第三条'), '多行字符串里的连续空行被压掉了');
});

test('codex：文件不存在时新建出一份可用的配置', (t) => {
  const dir = makeDir(t);
  applyClientConfig(dir, 'codex', CONTEXT);
  const text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf8');
  assert.match(text, /^model_provider = "ccproxy"\n\n\[model_providers\.ccproxy\]\n/);
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：只有顶层键、一个节都没有的文件也能补出 model_provider', () => {
  const text = rewriteCodexConfig('model = "gpt-5.6-sol"\n', CONTEXT);
  assert.match(text, /^model_provider = "ccproxy"\nmodel = "gpt-5\.6-sol"\n\n\[model_providers\.ccproxy\]\n/);
});

test('codex：写完留一份 .ccproxy-bak', (t) => {
  const dir = makeDir(t);
  const filePath = writeFile(dir, 'config.toml', 'model_provider = "custom"\n');
  const result = applyClientConfig(dir, 'codex', CONTEXT);
  assert.equal(result[0].backupPath, `${filePath}.ccproxy-bak`);
  assert.equal(fs.readFileSync(result[0].backupPath, 'utf8'), 'model_provider = "custom"\n');
  assert.deepEqual(fs.readdirSync(dir).sort(), ['config.toml', 'config.toml.ccproxy-bak']);
});

test('未知目录类型不写任何东西', (t) => {
  const dir = makeDir(t);
  assert.throws(() => applyClientConfig(dir, 'gemini', CONTEXT), /未知的目录类型/);
  assert.deepEqual(fs.readdirSync(dir), []);
});

test('token 为空时拒绝写入：写进去会把客户端的认证一起写坏', (t) => {
  const dir = makeDir(t);
  for (const kind of ['claude', 'codex']) {
    assert.throws(() => applyClientConfig(dir, kind, { port: 18888, token: '' }), /还没有客户端 token/);
  }
  assert.deepEqual(fs.readdirSync(dir), []);
});
