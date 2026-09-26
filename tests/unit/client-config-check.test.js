// 客户端配置检查单测：claude 的 settings.json、codex 的 config.toml，
// 以及「指向本机但不是 CCProxy 端口」这种必须区分开的情况
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { inspectClientConfig, isCcproxyAddress } = require('../../electron/services/clients/check');

const CONTEXT = { port: 18888, token: 't_abc123' };

// 每个用例一个临时目录（写在 /tmp 下，不污染项目）
function makeDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-check-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function claudeDir(t, env) {
  const dir = makeDir(t);
  fs.writeFileSync(path.join(dir, 'settings.json'), JSON.stringify({ env, permissions: { allow: ['Bash(ls:*)'] } }, null, 2));
  return dir;
}

function codexDir(t, content) {
  const dir = makeDir(t);
  fs.writeFileSync(path.join(dir, 'config.toml'), content);
  return dir;
}

function codexConfig({ providerId = 'custom', baseUrl, token, requiresOpenaiAuth = null, extraSection = '' }) {
  return [
    `model_provider = "${providerId}"`,
    'model = "ccproxy"',
    'model_reasoning_effort = "medium"',
    '',
    `[model_providers.${providerId}]`,
    `name = "${providerId}"`,
    'wire_api = "responses"',
    ...(requiresOpenaiAuth === null ? [] : [`requires_openai_auth = ${requiresOpenaiAuth}`]),
    ...(baseUrl ? [`base_url = "${baseUrl}"`] : []),
    ...(token ? [`experimental_bearer_token = "${token}"`] : []),
    ...(extraSection ? ['', extraSection] : [])
  ].join('\n');
}

test('地址判定：只有本机 + CCProxy 当前端口才算', () => {
  assert.equal(isCcproxyAddress('http://127.0.0.1:18888', 18888), true);
  assert.equal(isCcproxyAddress('http://localhost:18888/v1', 18888), true);
  assert.equal(isCcproxyAddress('http://[::1]:18888', 18888), true);
  // 本机但端口不是 CCProxy 的（CCGUI 的本地代理就挂在别的端口上）
  assert.equal(isCcproxyAddress('http://127.0.0.1:15721', 18888), false);
  // 不写端口 = 默认 80/443，不可能等于我们的端口
  assert.equal(isCcproxyAddress('http://127.0.0.1', 18888), false);
  // 远端地址与不是 URL 的写法
  assert.equal(isCcproxyAddress('http://192.168.60.2:18888', 18888), false);
  assert.equal(isCcproxyAddress('127.0.0.1:18888', 18888), false);
  assert.equal(isCcproxyAddress('', 18888), false);
});

test('claude：地址与 token 都对 → ok', (t) => {
  const dir = claudeDir(t, { ANTHROPIC_BASE_URL: 'http://127.0.0.1:18888', ANTHROPIC_AUTH_TOKEN: 't_abc123' });
  assert.deepEqual(inspectClientConfig(dir, 'claude', CONTEXT), {
    status: 'ok',
    target: 'http://127.0.0.1:18888',
    problems: []
  });
});

test('claude：base_url 带 /v1 也认（路径不影响转发）', (t) => {
  const dir = claudeDir(t, { ANTHROPIC_BASE_URL: 'http://127.0.0.1:18888/v1', ANTHROPIC_AUTH_TOKEN: 't_abc123' });
  assert.equal(inspectClientConfig(dir, 'claude', CONTEXT).status, 'ok');
});

test('claude：指向本机别的端口 → mismatch，问题里带上它实际指向哪', (t) => {
  const dir = claudeDir(t, { ANTHROPIC_BASE_URL: 'http://127.0.0.1:15721', ANTHROPIC_AUTH_TOKEN: 't_abc123' });
  const result = inspectClientConfig(dir, 'claude', CONTEXT);
  assert.equal(result.status, 'mismatch');
  assert.equal(result.target, 'http://127.0.0.1:15721');
  assert.deepEqual(result.problems, ['ANTHROPIC_BASE_URL 指向 http://127.0.0.1:15721，不是 CCProxy（http://127.0.0.1:18888）']);
});

test('claude：地址对但 token 不对 → mismatch；两样都缺 → 两条问题', (t) => {
  const badToken = claudeDir(t, { ANTHROPIC_BASE_URL: 'http://127.0.0.1:18888', ANTHROPIC_AUTH_TOKEN: 'other' });
  assert.deepEqual(inspectClientConfig(badToken, 'claude', CONTEXT).problems, ['ANTHROPIC_AUTH_TOKEN token 与 CCProxy 的客户端 token 不一致']);

  const empty = claudeDir(t, {});
  assert.deepEqual(inspectClientConfig(empty, 'claude', CONTEXT).problems, [
    '没有配置 ANTHROPIC_BASE_URL',
    'ANTHROPIC_AUTH_TOKEN 没有配置 token'
  ]);
});

test('claude：文件读不出来 / 不是合法 JSON / 根节点不是对象 → unreadable', (t) => {
  const missing = makeDir(t);
  assert.equal(inspectClientConfig(missing, 'claude', CONTEXT).status, 'unreadable');

  const broken = makeDir(t);
  fs.writeFileSync(path.join(broken, 'settings.json'), '{ 不是 json');
  assert.match(inspectClientConfig(broken, 'claude', CONTEXT).problems[0], /不是合法 JSON/);

  const array = makeDir(t);
  fs.writeFileSync(path.join(array, 'settings.json'), '[1,2]');
  assert.match(inspectClientConfig(array, 'claude', CONTEXT).problems[0], /根节点不是对象/);
});

test('codex：model_provider 指向的块里地址与 token 都对 → ok', (t) => {
  const dir = codexDir(t, codexConfig({ baseUrl: 'http://127.0.0.1:18888/v1', token: 't_abc123', requiresOpenaiAuth: false }));
  assert.deepEqual(inspectClientConfig(dir, 'codex', CONTEXT), {
    status: 'ok',
    target: 'http://127.0.0.1:18888/v1',
    problems: []
  });
});

test('codex：看的是 model_provider 指向的那一块，不是别的 provider 块', (t) => {
  const dir = codexDir(t, [
    'model_provider = "ccswitch"',
    '',
    '[model_providers.ccgui_model_1774018529128]',
    'name = "codexcn"',
    'base_url = "https://api2.codexcn.com/v1"',
    '',
    '[model_providers.ccswitch]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：节名带行尾注释、或用引号包起来，照样能和 model_provider 对上', (t) => {
  const commented = codexDir(t, [
    'model_provider = "ccproxy"',
    '',
    '[model_providers.ccproxy] # CCProxy 写入的',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));
  assert.equal(inspectClientConfig(commented, 'codex', CONTEXT).status, 'ok');

  // TOML 里 [model_providers."ccproxy"] 与 [model_providers.ccproxy] 是同一个表
  const quoted = codexDir(t, [
    'model_provider = "ccproxy"',
    '',
    '[model_providers."ccproxy"]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));
  assert.equal(inspectClientConfig(quoted, 'codex', CONTEXT).status, 'ok');
});

test('codex：三引号没闭合 → unreadable，并说明原因', (t) => {
  const dir = codexDir(t, 'model_provider = "custom"\ndev = """\n这里没闭合\n');
  const result = inspectClientConfig(dir, 'codex', CONTEXT);
  assert.equal(result.status, 'unreadable');
  assert.match(result.problems[0], /多行字符串没闭合/);
});

test('codex：model_provider 没配或指向不存在的块 → unreadable', (t) => {
  const noProvider = codexDir(t, 'model = "ccproxy"\n[features]\nmulti_agent = true\n');
  assert.match(inspectClientConfig(noProvider, 'codex', CONTEXT).problems[0], /没有配置 model_provider/);

  const ghost = codexDir(t, 'model_provider = "ghost"\n\n[model_providers.other]\nbase_url = "http://127.0.0.1:18888"\n');
  assert.match(inspectClientConfig(ghost, 'codex', CONTEXT).problems[0], /没有 \[model_providers\.ghost\] 块/);
});

test('codex：requires_openai_auth = true 时 token 由 auth.json 提供，单独报出来', (t) => {
  const dir = codexDir(t, codexConfig({ baseUrl: 'http://127.0.0.1:18888/v1', token: 't_abc123', requiresOpenaiAuth: true }));
  const result = inspectClientConfig(dir, 'codex', CONTEXT);
  assert.equal(result.status, 'mismatch');
  // 地址是对的，但凭据不走这个 token，不能算通过；也不该再多报一条「token 不一致」混淆视听
  assert.deepEqual(result.problems, ['requires_openai_auth 为 true，codex 会用 auth.json 里的凭据而不是这个 token']);
});

test('codex：三引号多行字符串里的内容不会被当成顶层键', (t) => {
  const dir = codexDir(t, [
    'model_provider = "custom"',
    'developer_instructions = """',
    'model_provider = "hijacked"',
    '"""',
    '',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：注释行与空值不干扰解析', (t) => {
  const dir = codexDir(t, [
    '# model_provider = "commented"',
    'model_provider = "custom"',
    'model_reasoning_effort = "medium"',
    '',
    '# [model_providers.other]',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('codex：行尾注释不影响判定（TOML 允许行尾注释，剥不掉会把注释当成值）', (t) => {
  const dir = codexDir(t, [
    'model_provider = "custom" # 走本地代理',
    '',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1" # 本地代理',
    'experimental_bearer_token = "t_abc123" # 填 CCProxy 的'
  ].join('\n'));
  assert.deepEqual(inspectClientConfig(dir, 'codex', CONTEXT), {
    status: 'ok',
    target: 'http://127.0.0.1:18888/v1',
    problems: []
  });
});

test('codex：单引号三引号块里的内容不算顶层键（只认 """ 会让 model_provider 被劫持）', (t) => {
  const dir = codexDir(t, [
    'model_provider = "custom"',
    "developer_instructions = '''",
    'model_provider = "hijacked"',
    "'''",
    '',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));
  assert.equal(inspectClientConfig(dir, 'codex', CONTEXT).status, 'ok');
});

test('地址判定：协议的默认端口要补回来（URL 规范化会把 :80 吃掉）', () => {
  assert.equal(isCcproxyAddress('http://127.0.0.1', 80), true);
  assert.equal(isCcproxyAddress('http://127.0.0.1:80', 80), true);
  assert.equal(isCcproxyAddress('https://localhost', 443), true);
  // 补默认端口不能把别的端口一起放进来
  assert.equal(isCcproxyAddress('http://127.0.0.1:8080', 80), false);
  assert.equal(isCcproxyAddress('https://localhost:80', 443), false);
});

test('codex：__proto__ 这类节名不会写到 Object.prototype 上', (t) => {
  const dir = codexDir(t, [
    'model_provider = "__proto__"',
    '',
    '[model_providers.__proto__]',
    'base_url = "http://127.0.0.1:18888/v1"',
    'experimental_bearer_token = "t_abc123"'
  ].join('\n'));

  const result = inspectClientConfig(dir, 'codex', CONTEXT);
  assert.equal(result.status, 'ok');
  assert.equal(Object.prototype.base_url, undefined);
  assert.equal(Object.prototype.experimental_bearer_token, undefined);
});

test('未知的目录类型直接报出来，不当成通过', (t) => {
  const dir = makeDir(t);
  const result = inspectClientConfig(dir, 'gemini', CONTEXT);
  assert.equal(result.status, 'unreadable');
  assert.match(result.problems[0], /未知的目录类型/);
});
