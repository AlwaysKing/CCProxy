// 供应商模型拉取单测：协议认证头、URL 拼接去重、响应解析、错误与超时场景
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('http');
const { fetchUpstreamModels } = require('../../electron/services/proxy/fetch-models');

function startMockServer(handler) {
  return new Promise((resolve) => {
    const server = http.createServer(handler);
    server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port }));
  });
}
function stopServer(server) {
  return new Promise((resolve) => server.close(() => resolve()));
}

test('anthropic 协议：发 x-api-key 与 anthropic-version，解析 data[].id 并去重', async () => {
  let seen = null;
  const mock = await startMockServer((req, res) => {
    seen = { url: req.url, headers: req.headers };
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ data: [
      { type: 'model', id: 'glm-4.6' },
      { type: 'model', id: 'glm-4.5-air' },
      { type: 'model', id: 'glm-4.6' }
    ] }));
  });

  const result = await fetchUpstreamModels({ protocolId: 'anthropic', baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-real' });
  assert.equal(result.ok, true);
  assert.deepEqual(result.models, ['glm-4.6', 'glm-4.5-air']);
  assert.equal(seen.url, '/v1/models');
  assert.equal(seen.headers['x-api-key'], 'sk-real');
  assert.equal(seen.headers['anthropic-version'], '2023-06-01');

  await stopServer(mock.server);
});

test('openai 协议：发 Authorization Bearer；baseUrl 带 /v1 时不重复拼接', async () => {
  let seen = null;
  const mock = await startMockServer((req, res) => {
    seen = { url: req.url, headers: req.headers };
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ object: 'list', data: [{ id: 'deepseek-chat', object: 'model' }] }));
  });

  const result = await fetchUpstreamModels({ protocolId: 'openai', baseUrl: `http://127.0.0.1:${mock.port}/v1`, token: 'sk-openai' });
  assert.equal(result.ok, true);
  assert.deepEqual(result.models, ['deepseek-chat']);
  assert.equal(seen.url, '/v1/models');
  assert.equal(seen.headers.authorization, 'Bearer sk-openai');

  await stopServer(mock.server);
});

test('token 为空时不带认证头', async () => {
  let seen = null;
  const mock = await startMockServer((req, res) => {
    seen = req.headers;
    res.writeHead(200).end(JSON.stringify({ data: [{ id: 'm1' }] }));
  });

  const result = await fetchUpstreamModels({ protocolId: 'openai', baseUrl: `http://127.0.0.1:${mock.port}`, token: '' });
  assert.equal(result.ok, true);
  assert.equal(seen.authorization, undefined);

  await stopServer(mock.server);
});

test('非 200 响应返回失败并带状态码', async () => {
  const mock = await startMockServer((req, res) => {
    res.writeHead(401).end(JSON.stringify({ error: 'bad key' }));
  });
  const result = await fetchUpstreamModels({ protocolId: 'openai', baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-x' });
  assert.equal(result.ok, false);
  assert.match(result.error, /HTTP 401/);

  await stopServer(mock.server);
});

test('响应不是可识别的模型列表结构时返回失败', async () => {
  const mock = await startMockServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' }).end('<html>not json</html>');
  });
  const result = await fetchUpstreamModels({ protocolId: 'openai', baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-x' });
  assert.equal(result.ok, false);
  assert.match(result.error, /可识别/);

  await stopServer(mock.server);
});

test('上游不可达时返回失败而不是抛异常', async () => {
  const result = await fetchUpstreamModels({ protocolId: 'openai', baseUrl: 'http://127.0.0.1:1', token: 'sk-x' });
  assert.equal(result.ok, false);
  assert.ok(result.error);
});

test('非法地址直接返回失败', async () => {
  const result = await fetchUpstreamModels({ protocolId: 'openai', baseUrl: 'not-a-url', token: 'sk-x' });
  assert.equal(result.ok, false);
  assert.match(result.error, /无效/);
});

test('未知协议返回失败而不是抛异常', async () => {
  const result = await fetchUpstreamModels({ protocolId: 'gemini', baseUrl: 'http://127.0.0.1:9', token: 'sk-x' });
  assert.equal(result.ok, false);
  assert.match(result.error, /未知协议/);
});
