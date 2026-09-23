// 模型列表接口单测：路径匹配、协议判定、可用映射过滤、两种协议的响应格式
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { matchModelsRequest, resolveClientProtocol, buildModelsResponse, usableClientModels } = require('../../electron/services/proxy/models');

function buildConfig(overrides) {
  // 深拷贝一份基础配置，允许每个用例独立改动
  const config = {
    protocols: [
      {
        id: 'anthropic', name: 'Anthropic', enabled: true,
        providers: [
          { id: 'p1', name: '供应商一', enabled: true, baseUrl: 'http://upstream-a.test', token: 't1', models: ['m1'] },
          { id: 'p2', name: '供应商二', enabled: false, baseUrl: 'http://upstream-b.test', token: 't2', models: ['m2'] }
        ],
        mappings: [
          { id: 'm-a', enabled: true, clientModel: 'claude-opus-4', providerId: 'p1', realModel: 'glm-4.6' },
          { id: 'm-b', enabled: true, clientModel: 'claude-sonnet-4', providerId: 'p1', realModel: 'glm-4.5' },
          { id: 'm-off', enabled: false, clientModel: 'claude-haiku-4', providerId: 'p1', realModel: 'x' },
          { id: 'm-dead-provider', enabled: true, clientModel: 'claude-3', providerId: 'p2', realModel: 'x' },
          { id: 'm-dup', enabled: true, clientModel: 'claude-opus-4', providerId: 'p1', realModel: 'x' },
          { id: 'm-wild', enabled: true, clientModel: 'claude-sonnet-4-*', providerId: 'p1', realModel: 'x' },
          { id: 'm-fallback', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'f1' }
        ]
      },
      { id: 'openai', name: 'OpenAI', enabled: false, providers: [], mappings: [] }
    ]
  };
  return overrides ? overrides(config) : config;
}

test('matchModelsRequest：只认 GET /v1/models 与 /v1/models/{id}', () => {
  assert.deepEqual(matchModelsRequest('GET', '/v1/models'), { modelId: null });
  assert.deepEqual(matchModelsRequest('GET', '/v1/models?limit=20'), { modelId: null });
  assert.deepEqual(matchModelsRequest('GET', '/v1/models/claude-opus-4'), { modelId: 'claude-opus-4' });
  assert.deepEqual(matchModelsRequest('GET', '/v1/models/%E6%A8%A1%E5%9E%8B'), { modelId: '模型' });
  // 非法百分号编码不抛异常，按原始字符串处理（后续走未命中 → 404）
  assert.deepEqual(matchModelsRequest('GET', '/v1/models/%E4%B8'), { modelId: '%E4%B8' });
  assert.equal(matchModelsRequest('POST', '/v1/models'), null);
  assert.equal(matchModelsRequest('GET', '/v1/messages'), null);
  assert.equal(matchModelsRequest('GET', '/v1/models/a/b'), null);
});

test('resolveClientProtocol：anthropic 头优先，否则按 openai 处理', () => {
  assert.equal(resolveClientProtocol({ 'anthropic-version': '2023-06-01' }), 'anthropic');
  assert.equal(resolveClientProtocol({ 'x-api-key': 'sk-fake' }), 'anthropic');
  assert.equal(resolveClientProtocol({ authorization: 'Bearer sk-fake' }), 'openai');
  assert.equal(resolveClientProtocol({}), 'openai');
});

test('usableClientModels：只留可用映射，跳过停用/供应商不可用/含 * 通配/重复', () => {
  assert.deepEqual(usableClientModels(buildConfig().protocols[0]), ['claude-opus-4', 'claude-sonnet-4']);
});

test('buildModelsResponse：anthropic 列表格式', () => {
  const { status, payload } = buildModelsResponse(buildConfig(), 'anthropic', null);
  assert.equal(status, 200);
  assert.deepEqual(payload.data.map((m) => m.id), ['claude-opus-4', 'claude-sonnet-4']);
  assert.ok(payload.data.every((m) => m.type === 'model' && m.display_name === m.id && typeof m.created_at === 'string'));
  assert.equal(payload.first_id, 'claude-opus-4');
  assert.equal(payload.last_id, 'claude-sonnet-4');
  assert.equal(payload.has_more, false);
});

test('buildModelsResponse：openai 列表格式', () => {
  const config = buildConfig((c) => {
    c.protocols[1].enabled = true;
    c.protocols[1].providers = [{ id: 'q1', name: '供应商', enabled: true, baseUrl: 'http://upstream-c.test', token: 't', models: ['m'] }];
    c.protocols[1].mappings = [{ id: 'm-o1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'deepseek-chat' }];
    return c;
  });
  const { status, payload } = buildModelsResponse(config, 'openai', null);
  assert.equal(status, 200);
  assert.equal(payload.object, 'list');
  assert.deepEqual(payload.data.map((m) => m.id), ['gpt-4o']);
  assert.ok(payload.data.every((m) => m.object === 'model' && m.owned_by === 'ccproxy' && Number.isInteger(m.created)));
});

test('buildModelsResponse：单模型查询命中返回单项', () => {
  const { status, payload } = buildModelsResponse(buildConfig(), 'anthropic', 'claude-opus-4');
  assert.equal(status, 200);
  assert.equal(payload.id, 'claude-opus-4');
  assert.equal(payload.type, 'model');
});

test('buildModelsResponse：单模型查询未命中按协议返回 404', () => {
  const anthropic = buildModelsResponse(buildConfig(), 'anthropic', 'no-such-model');
  assert.equal(anthropic.status, 404);
  assert.equal(anthropic.payload.error.type, 'not_found_error');

  const openai = buildModelsResponse(buildConfig((c) => { c.protocols[1].enabled = true; return c; }), 'openai', 'no-such-model');
  assert.equal(openai.status, 404);
  assert.equal(openai.payload.error.code, 'model_not_found');
});

test('buildModelsResponse：协议停用返回 503，协议不存在返回 404', () => {
  const disabled = buildModelsResponse(buildConfig(), 'openai', null);
  assert.equal(disabled.status, 503);
  assert.equal(disabled.payload.error.type, 'protocol_disabled');

  const missing = buildModelsResponse(buildConfig(), 'nope', null);
  assert.equal(missing.status, 404);
  assert.equal(missing.payload.error.type, 'protocol_not_found');
});
