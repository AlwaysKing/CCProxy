// 映射查表：在指定协议的映射表里为客户端模型名找到目标供应商与真实模型名。
// 优先级：精确匹配 > 通配匹配（前缀最长者优先）> 兜底规则（clientModel 为 *）。
// 启停规则：协议停用报 503；供应商或单条映射停用的条目视为不可用直接跳过。

'use strict';

function isWildcardPattern(pattern) {
  return typeof pattern === 'string' && pattern.length > 1 && pattern.endsWith('*');
}

// 单条规则能否匹配客户端模型名（精确 / 通配 / 兜底三种形态）
function matchMapping(mapping, clientModel) {
  const pattern = mapping.clientModel;
  if (typeof pattern !== 'string' || pattern.length === 0) return false;
  if (pattern === '*') return true;
  if (pattern === clientModel) return true;
  if (isWildcardPattern(pattern)) return clientModel.startsWith(pattern.slice(0, -1));
  return false;
}

function routingError(status, code, message) {
  return { error: { status, code, message } };
}

// 查表入口。config 为应用配置对象（结构与 ~/.ccproxy/app.json 一致）
function resolveRouting(config, protocolId, clientModel) {
  const protocol = (config && Array.isArray(config.protocols) ? config.protocols : []).find((p) => p.id === protocolId);
  if (!protocol) {
    return routingError(404, 'protocol_not_found', `配置中不存在协议 ${protocolId}`);
  }
  if (!protocol.enabled) {
    return routingError(503, 'protocol_disabled', `协议 ${protocol.name}（${protocol.id}）已停用`);
  }

  const providers = Array.isArray(protocol.providers) ? protocol.providers : [];
  const mappings = Array.isArray(protocol.mappings) ? protocol.mappings : [];

  // 只保留可用条目：映射启用、供应商存在且启用
  const usable = [];
  for (const mapping of mappings) {
    if (!mapping || !mapping.enabled) continue;
    const provider = providers.find((p) => p.id === mapping.providerId);
    if (!provider || !provider.enabled) continue;
    usable.push({ mapping, provider });
  }

  // 三轮各记一个 matchKind：请求日志要写「命中规则」，而「是哪一轮命中的」只有这里知道。
  // 让渲染层从模式串反推（判断含不含 *）等于同一条规则写两份，早晚漂移
  // 第一轮：精确匹配
  let hit = usable.find((e) => e.mapping.clientModel === clientModel);
  let matchKind = hit ? 'exact' : null;
  // 第二轮：通配，前缀最长者优先
  if (!hit) {
    const wildcards = usable
      .filter((e) => isWildcardPattern(e.mapping.clientModel) && matchMapping(e.mapping, clientModel))
      .sort((a, b) => (b.mapping.clientModel.length - a.mapping.clientModel.length));
    hit = wildcards[0];
    matchKind = hit ? 'wildcard' : null;
  }
  // 第三轮：兜底
  if (!hit) {
    hit = usable.find((e) => e.mapping.clientModel === '*');
    matchKind = hit ? 'fallback' : null;
  }

  if (!hit) {
    return routingError(404, 'mapping_not_found', `模型 ${clientModel || '(空)'} 未在 ${protocol.name} 协议映射表中配置`);
  }
  return {
    protocolId,
    provider: { id: hit.provider.id, name: hit.provider.name, baseUrl: hit.provider.baseUrl, token: hit.provider.token },
    mapping: hit.mapping,
    matchKind,
    realModel: hit.mapping.realModel
  };
}

module.exports = { resolveRouting, matchMapping };
