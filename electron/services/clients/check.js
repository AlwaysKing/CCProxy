// 客户端配置检查：目录里的配置文件是不是指向 CCProxy。
// claude 读 settings.json 的 env.ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN；
// codex 读 config.toml 里 model_provider 指向的那个 [model_providers.<id>] 块的
// base_url / experimental_bearer_token。
// 只依赖 fs/os/path，不 require electron，可以脱离主进程单测。

'use strict';

const fs = require('fs');
const path = require('path');
const { scanTomlLines } = require('./toml');

// 认作「本机」的主机名。地址必须搭配端口一起判：CCGUI 的本地代理也挂在本机，
// 只看主机名会把它的配置当成我们的
const LOOPBACK_HOSTS = ['127.0.0.1', 'localhost', '::1', '[::1]'];
// URL 规范化会把协议的默认端口吃掉（http://host:80 → port 为空串），比较时补回来，
// 否则界面写着「期望 http://127.0.0.1:80」而检查永远不认
const DEFAULT_PORTS = { 'http:': '80', 'https:': '443' };
// provider 块的表名前缀（apply 那边写的是同一个，两侧靠「写完再检查一次」互通）
const PROVIDER_SECTION_PREFIX = 'model_providers.';

function isPlainObject(value) {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function isCcproxyAddress(rawUrl, port) {
  try {
    const url = new URL(String(rawUrl).trim());
    if (!LOOPBACK_HOSTS.includes(url.hostname)) return false;
    // 只比主机与端口，不比路径：base_url 带不带 /v1 转发都能对上（上游拼接会去重）
    return (url.port || DEFAULT_PORTS[url.protocol] || '') === String(port);
  } catch {
    // 不是合法 URL（例如只写了主机端口没写协议）：报给用户看原文，比猜他的意思强
    return false;
  }
}

function collectAddressProblem(baseUrl, context, label) {
  if (!baseUrl) return `没有配置 ${label}`;
  if (!isCcproxyAddress(baseUrl, context.port)) {
    return `${label} 指向 ${baseUrl}，不是 CCProxy（${context.address}）`;
  }
  return '';
}

function collectTokenProblem(token, context) {
  if (!token) return '没有配置 token';
  if (token !== context.token) return 'token 与 CCProxy 的客户端 token 不一致';
  return '';
}

function readClaudeEndpoint(dirPath) {
  const settingsPath = path.join(dirPath, 'settings.json');
  let text;
  try {
    text = fs.readFileSync(settingsPath, 'utf8');
  } catch (error) {
    return { error: `读取 settings.json 失败：${error.message}` };
  }
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    return { error: `settings.json 不是合法 JSON：${error.message}` };
  }
  if (!isPlainObject(parsed)) return { error: 'settings.json 的根节点不是对象' };
  const env = isPlainObject(parsed.env) ? parsed.env : {};
  return {
    baseUrl: typeof env.ANTHROPIC_BASE_URL === 'string' ? env.ANTHROPIC_BASE_URL.trim() : '',
    token: typeof env.ANTHROPIC_AUTH_TOKEN === 'string' ? env.ANTHROPIC_AUTH_TOKEN.trim() : ''
  };
}

function readCodexProvider(dirPath) {
  const configPath = path.join(dirPath, 'config.toml');
  let text;
  try {
    text = fs.readFileSync(configPath, 'utf8');
  } catch (error) {
    return { error: `读取 config.toml 失败：${error.message}` };
  }

  const { lines, unterminated } = scanTomlLines(text);
  // 三引号没闭合的文件已经读不动了（codex 自己也解析不了），别硬猜里面是什么
  if (unterminated) return { error: 'config.toml 里有多行字符串没闭合（三引号只有一个）' };

  // 无原型对象：节名可以是 __proto__ 这类名字，用普通对象当字典会被写到 Object.prototype 上
  const providers = Object.create(null);
  let providerId = '';
  for (const line of lines) {
    if (line.kind !== 'entry') continue;
    if (!line.section) {
      if (line.key === 'model_provider') providerId = line.value;
      continue;
    }
    // 节名已归一化，[model_providers."ccproxy"] 与 [model_providers.ccproxy] 是同一个
    if (!line.section.startsWith(PROVIDER_SECTION_PREFIX)) continue;
    const id = line.section.slice(PROVIDER_SECTION_PREFIX.length);
    if (!providers[id]) providers[id] = Object.create(null);
    providers[id][line.key] = line.value;
  }

  if (!providerId) return { error: 'config.toml 没有配置 model_provider' };
  const block = providers[providerId];
  if (!block) return { error: `model_provider 指向 ${providerId}，但配置里没有 [model_providers.${providerId}] 块` };

  return {
    providerId,
    baseUrl: block.base_url || '',
    token: block.experimental_bearer_token || '',
    requiresOpenaiAuth: block.requires_openai_auth === 'true'
  };
}

function inspectClaudeDir(dirPath, context) {
  const endpoint = readClaudeEndpoint(dirPath);
  if (endpoint.error) return { status: 'unreadable', target: '', problems: [endpoint.error] };

  const problems = [];
  const addressProblem = collectAddressProblem(endpoint.baseUrl, context, 'ANTHROPIC_BASE_URL');
  if (addressProblem) problems.push(addressProblem);
  const tokenProblem = collectTokenProblem(endpoint.token, context);
  if (tokenProblem) problems.push(`ANTHROPIC_AUTH_TOKEN ${tokenProblem}`);
  return { status: problems.length ? 'mismatch' : 'ok', target: endpoint.baseUrl, problems };
}

function inspectCodexDir(dirPath, context) {
  const provider = readCodexProvider(dirPath);
  if (provider.error) return { status: 'unreadable', target: '', problems: [provider.error] };

  const problems = [];
  const addressProblem = collectAddressProblem(provider.baseUrl, context, 'base_url');
  if (addressProblem) problems.push(addressProblem);
  // requires_openai_auth = true 时 codex 送的是 auth.json 里的 ChatGPT 凭据，
  // 不是配置里的 token——这种配置过不了 CCProxy 的入站校验，得指出来
  if (provider.requiresOpenaiAuth) {
    problems.push('requires_openai_auth 为 true，codex 会用 auth.json 里的凭据而不是这个 token');
  } else {
    const tokenProblem = collectTokenProblem(provider.token, context);
    if (tokenProblem) problems.push(`experimental_bearer_token ${tokenProblem}`);
  }
  return { status: problems.length ? 'mismatch' : 'ok', target: provider.baseUrl, problems };
}

// 检查一个目录。context = { port, token }，address 由 port 推出，给界面显示「应该指向哪」
function inspectClientConfig(dirPath, kind, { port, token }) {
  const context = { port, token, address: `http://127.0.0.1:${port}` };
  if (kind === 'claude') return inspectClaudeDir(dirPath, context);
  if (kind === 'codex') return inspectCodexDir(dirPath, context);
  return { status: 'unreadable', target: '', problems: [`未知的目录类型：${kind}`] };
}

module.exports = { inspectClientConfig, isCcproxyAddress, readClaudeEndpoint, readCodexProvider };
