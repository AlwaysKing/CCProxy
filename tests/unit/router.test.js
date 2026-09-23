// 映射查表单测：优先级（精确 > 通配最长前缀 > 兜底）、启停跳过、错误场景
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { resolveRouting } = require('../../electron/services/proxy/router');

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
          { id: 'exact', enabled: true, clientModel: 'claude-sonnet-4', providerId: 'p1', realModel: 'm1' },
          { id: 'wild-short', enabled: true, clientModel: 'claude-*', providerId: 'p1', realModel: 'w1' },
          { id: 'wild-long', enabled: true, clientModel: 'claude-3-5-*', providerId: 'p2', realModel: 'w2' },
          { id: 'fallback', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'f1' }
        ]
      },
      { id: 'openai', name: 'OpenAI', enabled: false, providers: [], mappings: [] }
    ]
  };
  return overrides ? overrides(config) : config;
}

test('精确匹配优先于通配与兜底', () => {
  const r = resolveRouting(buildConfig(), 'anthropic', 'claude-sonnet-4');
  assert.equal(r.mapping.id, 'exact');
  assert.equal(r.realModel, 'm1');
  assert.equal(r.provider.id, 'p1');
});

test('通配按前缀最长优先', () => {
  const r = resolveRouting(buildConfig(), 'anthropic', 'claude-3-5-haiku');
  // wild-long 前缀更长，但它指向的供应商 p2 已停用 → 跳过，落给 wild-short
  assert.equal(r.mapping.id, 'wild-short');
  assert.equal(r.realModel, 'w1');
});

test('供应商停用的映射条目视为不可用', () => {
  const config = buildConfig((c) => {
    c.protocols[0].providers[0].enabled = false;
    c.protocols[0].mappings = c.protocols[0].mappings.filter((m) => m.id !== 'fallback');
    return c;
  });
  const r = resolveRouting(config, 'anthropic', 'claude-sonnet-4');
  assert.ok(r.error);
  assert.equal(r.error.status, 404);
});

test('停用的映射条目被跳过', () => {
  const config = buildConfig((c) => {
    c.protocols[0].mappings.find((m) => m.id === 'exact').enabled = false;
    return c;
  });
  const r = resolveRouting(config, 'anthropic', 'claude-sonnet-4');
  assert.equal(r.mapping.id, 'wild-short');
});

test('未命中任何规则时落到兜底', () => {
  const r = resolveRouting(buildConfig(), 'anthropic', 'totally-unknown');
  assert.equal(r.mapping.id, 'fallback');
  assert.equal(r.realModel, 'f1');
});

test('无兜底且未命中时报 404，报错文案含模型名与协议名', () => {
  const config = buildConfig((c) => {
    c.protocols[0].mappings = c.protocols[0].mappings.filter((m) => m.id !== 'fallback');
    return c;
  });
  const r = resolveRouting(config, 'anthropic', 'totally-unknown');
  assert.ok(r.error);
  assert.equal(r.error.status, 404);
  assert.ok(r.error.message.includes('totally-unknown'));
  assert.ok(r.error.message.includes('Anthropic'));
});

test('协议停用时报 503', () => {
  const r = resolveRouting(buildConfig(), 'openai', 'gpt-4o');
  assert.ok(r.error);
  assert.equal(r.error.status, 503);
});

test('映射指向不存在的供应商时报 404', () => {
  const config = buildConfig((c) => {
    c.protocols[0].mappings = [{ id: 'ghost', enabled: true, clientModel: 'claude-sonnet-4', providerId: 'nope', realModel: 'x' }];
    return c;
  });
  const r = resolveRouting(config, 'anthropic', 'claude-sonnet-4');
  assert.ok(r.error);
  assert.equal(r.error.status, 404);
});

test('配置里没有该协议时报 404', () => {
  const r = resolveRouting(buildConfig(), 'gemini', 'gemini-pro');
  assert.ok(r.error);
  assert.equal(r.error.status, 404);
});

test('matchKind 标明命中轮次：精确 exact / 通配 wildcard / 兜底 fallback', () => {
  // 界面的「精确/通配/兜底」徽章直接读这个字段，不在这里给全，渲染层就得自己从模式串反推
  assert.equal(resolveRouting(buildConfig(), 'anthropic', 'claude-sonnet-4').matchKind, 'exact');
  assert.equal(resolveRouting(buildConfig(), 'anthropic', 'claude-opus-9').matchKind, 'wildcard');
  assert.equal(resolveRouting(buildConfig(), 'anthropic', 'totally-other-model').matchKind, 'fallback');
});
