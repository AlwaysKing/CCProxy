// 上游转发单测：URL 拼接（含重复版本段去重）、出站头清洗
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { buildUpstreamUrl, sanitizeOutboundHeaders } = require('../../electron/services/proxy/upstream');

test('baseUrl 无版本段时直接拼接', () => {
  assert.equal(buildUpstreamUrl('https://api.anthropic.com', '/v1/messages'), 'https://api.anthropic.com/v1/messages');
});

test('baseUrl 已带 /v1 且路径也带 /v1 时去重', () => {
  assert.equal(buildUpstreamUrl('http://upstream.test:9005/v1', '/v1/responses'), 'http://upstream.test:9005/v1/responses');
});

test('baseUrl 带 /v1 而路径不带时正常拼接', () => {
  assert.equal(buildUpstreamUrl('http://upstream.test:9005/v1', '/chat/completions'), 'http://upstream.test:9005/v1/chat/completions');
});

test('baseUrl 末尾多余斜杠被清理', () => {
  assert.equal(buildUpstreamUrl('http://upstream.test/api/anthropic/', '/v1/messages'), 'http://upstream.test/api/anthropic/v1/messages');
});

test('出站头清洗：剥离 hop-by-hop 与重算头，保留其余', () => {
  const headers = {
    host: '127.0.0.1:18888',
    connection: 'keep-alive',
    'transfer-encoding': 'chunked',
    'content-length': '999',
    'accept-encoding': 'gzip',
    'content-type': 'application/json',
    'anthropic-version': '2023-06-01'
  };
  const outbound = sanitizeOutboundHeaders(headers);
  assert.equal(outbound.host, undefined);
  assert.equal(outbound.connection, undefined);
  assert.equal(outbound['transfer-encoding'], undefined);
  assert.equal(outbound['content-length'], undefined);
  assert.equal(outbound['accept-encoding'], undefined);
  assert.equal(outbound['content-type'], 'application/json');
  assert.equal(outbound['anthropic-version'], '2023-06-01');
});
