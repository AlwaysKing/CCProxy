// 协议适配器单测：入站凭据剥离、按协议注入认证、model 改写、响应 model 回写
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { getAdapter, stripInboundCredentials, createResponseRewriter } = require('../../electron/services/proxy/adapters');

test('剥离入站凭据：五类认证头全部摘除，其余保留', () => {
  const headers = {
    'content-type': 'application/json',
    authorization: 'Bearer fake',
    'x-api-key': 'fake',
    'api-key': 'fake',
    'x-goog-api-key': 'fake',
    'proxy-authorization': 'Basic fake',
    'anthropic-version': '2023-06-01',
    'user-agent': 'test'
  };
  const stripped = stripInboundCredentials(headers);
  assert.equal(stripped['content-type'], 'application/json');
  assert.equal(stripped['anthropic-version'], '2023-06-01');
  assert.equal(stripped['user-agent'], 'test');
  assert.equal(stripped.authorization, undefined);
  assert.equal(stripped['x-api-key'], undefined);
  assert.equal(stripped['api-key'], undefined);
  assert.equal(stripped['x-goog-api-key'], undefined);
  assert.equal(stripped['proxy-authorization'], undefined);
});

test('anthropic 适配器注入 x-api-key', () => {
  const adapter = getAdapter('anthropic');
  const headers = {};
  adapter.applyAuth(headers, 'sk-real');
  assert.equal(headers['x-api-key'], 'sk-real');
  assert.equal(headers.authorization, undefined);
});

test('openai 适配器注入 Authorization Bearer', () => {
  const adapter = getAdapter('openai');
  const headers = {};
  adapter.applyAuth(headers, 'sk-real');
  assert.equal(headers.authorization, 'Bearer sk-real');
  assert.equal(headers['x-api-key'], undefined);
});

test('两个适配器都改写请求体顶层 model 字段', () => {
  for (const id of ['anthropic', 'openai']) {
    const adapter = getAdapter(id);
    const payload = { model: 'claude-sonnet-4', messages: [{ role: 'user', content: 'hi' }] };
    adapter.rewriteModel(payload, 'glm-4.6');
    assert.equal(payload.model, 'glm-4.6');
    assert.deepEqual(payload.messages, [{ role: 'user', content: 'hi' }]);
  }
});

test('未知协议抛出明确异常', () => {
  assert.throws(() => getAdapter('gemini'), /未知协议/);
});

test('响应回写：非流式 JSON 顶层 model 换回客户端模型名', () => {
  const rewriter = createResponseRewriter('anthropic', 'cnsinda-glm-5.3');
  const out = rewriter.rewriteBody(Buffer.from(JSON.stringify({ id: 'msg_1', model: 'glm-5.3', content: [] })));
  const payload = JSON.parse(out.toString('utf8'));
  assert.equal(payload.model, 'cnsinda-glm-5.3');
  assert.equal(payload.id, 'msg_1');
});

test('响应回写：非 JSON 或没有 model 字段的响应原样透传', () => {
  const rewriter = createResponseRewriter('anthropic', 'cnsinda-glm-5.3');
  const notJson = Buffer.from('<html>upstream error page</html>', 'utf8');
  assert.equal(rewriter.rewriteBody(notJson).compare(notJson), 0);
  const noModel = Buffer.from(JSON.stringify({ error: { type: 'not_found_error' } }));
  assert.equal(rewriter.rewriteBody(noModel).compare(noModel), 0);
});

test('响应回写：anthropic 流式只改 message_start 里的 message.model', () => {
  const rewriter = createResponseRewriter('anthropic', 'cnsinda-glm-5.3');
  const event = Buffer.from('event: message_start\ndata: {"type":"message_start","message":{"model":"glm-5.3","role":"assistant"}}\n\n', 'utf8');
  const out = rewriter.rewriteSseEvent(event).toString('utf8');
  assert.ok(out.startsWith('event: message_start\n'));
  assert.ok(out.endsWith('\n\n'));
  assert.equal(JSON.parse(out.split('\n')[1].slice(5)).message.model, 'cnsinda-glm-5.3');

  // 非 message_start 事件不带 model，改写后内容不变
  const delta = Buffer.from('event: content_block_delta\ndata: {"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}\n\n', 'utf8');
  assert.equal(rewriter.rewriteSseEvent(delta).compare(delta), 0);
});

test('响应回写：openai 流式每个 data 行的顶层 model 都换回', () => {
  const rewriter = createResponseRewriter('openai', 'gpt-4o-proxy');
  const event = Buffer.from('data: {"id":"x","model":"deepseek-chat","choices":[]}\n\ndata: [DONE]\n\n', 'utf8');
  const out = rewriter.rewriteSseEvent(event).toString('utf8');
  const lines = out.split('\n');
  assert.equal(JSON.parse(lines[0].slice(5)).model, 'gpt-4o-proxy');
  assert.equal(lines[2], 'data: [DONE]');
});

test('响应回写：跨 chunk 的多字节文本不受事件切分影响（回写器拿到的一定是完整事件）', () => {
  const rewriter = createResponseRewriter('anthropic', 'cnsinda-glm-5.3');
  const text = '跑通';
  const event = Buffer.from(`event: content_block_delta\ndata: ${JSON.stringify({ type: 'content_block_delta', delta: { text } })}\n\n`, 'utf8');
  const out = JSON.parse(rewriter.rewriteSseEvent(event).toString('utf8').split('\n')[1].slice(5));
  assert.equal(out.delta.text, '跑通');
});
