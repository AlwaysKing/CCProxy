// 协议识别器单测：打分制在各种特征组合下的判定
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { detectProtocol } = require('../../electron/services/proxy/detector');

function anthropicBody() {
  return Buffer.from(JSON.stringify({ model: 'claude-sonnet-4-20250514', max_tokens: 1024, system: [{ type: 'text', text: 'x' }], messages: [] }));
}
function openaiBody() {
  return Buffer.from(JSON.stringify({ model: 'gpt-4o', messages: [], temperature: 1 }));
}

test('路径 /v1/messages 识别为 anthropic', () => {
  const r = detectProtocol({ method: 'POST', path: '/v1/messages', headers: {}, body: anthropicBody() });
  assert.equal(r.protocol, 'anthropic');
  assert.ok(r.scores.anthropic > r.scores.openai);
});

test('路径带查询串 /v1/messages?beta=true 识别为 anthropic', () => {
  const r = detectProtocol({ method: 'POST', path: '/v1/messages?beta=true', headers: {}, body: anthropicBody() });
  assert.equal(r.protocol, 'anthropic');
});

test('路径 /v1/chat/completions 识别为 openai', () => {
  const r = detectProtocol({ method: 'POST', path: '/v1/chat/completions', headers: {}, body: openaiBody() });
  assert.equal(r.protocol, 'openai');
});

test('路径 /v1/responses 识别为 openai', () => {
  const r = detectProtocol({ method: 'POST', path: '/v1/responses', headers: {}, body: openaiBody() });
  assert.equal(r.protocol, 'openai');
});

test('GET /v1/models 无请求体也能靠路径识别为 openai', () => {
  const r = detectProtocol({ method: 'GET', path: '/v1/models', headers: {}, body: null });
  assert.equal(r.protocol, 'openai');
});

test('无路径特征时靠请求头 anthropic-version 识别', () => {
  const r = detectProtocol({ method: 'POST', path: '/foo', headers: { 'anthropic-version': '2023-06-01' }, body: null });
  assert.equal(r.protocol, 'anthropic');
});

test('无路径无请求头时靠请求体特征识别为 anthropic', () => {
  const r = detectProtocol({ method: 'POST', path: '/', headers: {}, body: anthropicBody() });
  assert.equal(r.protocol, 'anthropic');
});

test('完全无特征时返回 null 并附得分与依据', () => {
  const r = detectProtocol({ method: 'POST', path: '/foo', headers: {}, body: Buffer.from('{}') });
  assert.equal(r.protocol, null);
  assert.deepEqual(r.scores, { anthropic: 0, openai: 0 });
  assert.ok(Array.isArray(r.reasons));
});

test('坏 JSON 请求体不会抛异常', () => {
  const r = detectProtocol({ method: 'POST', path: '/v1/messages', headers: {}, body: Buffer.from('not json') });
  assert.equal(r.protocol, 'anthropic');
});
