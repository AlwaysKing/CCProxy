// 模型列表接口接管：GET /v1/models 与 GET /v1/models/{id} 不透传上游，
// 由代理按当前激活的映射表直接生成响应，客户端看到的就是本代理实际可路由的模型名。
// 两个协议的该路径完全相同，打分制会平局，改按请求头判定客户端类型：
// 带 anthropic-version 或 x-api-key 视为 Anthropic 客户端，否则按 OpenAI 处理。

'use strict';

// 与 router.js 的可用性口径一致：映射启用、供应商存在且启用才算可路由
function usableClientModels(protocol) {
  const providers = Array.isArray(protocol.providers) ? protocol.providers : [];
  const mappings = Array.isArray(protocol.mappings) ? protocol.mappings : [];
  const names = [];
  for (const mapping of mappings) {
    if (!mapping || !mapping.enabled) continue;
    // 含 * 的通配（含兜底 *）不是真实模型名，不列入列表（请求时仍正常路由）
    if (typeof mapping.clientModel !== 'string' || mapping.clientModel === '' || mapping.clientModel.includes('*')) continue;
    const provider = providers.find((p) => p.id === mapping.providerId);
    if (!provider || !provider.enabled) continue;
    if (!names.includes(mapping.clientModel)) names.push(mapping.clientModel);
  }
  return names;
}

// 是否为代理接管的模型列表请求；是则返回 { modelId: null | 'xxx' }，否则 null
function matchModelsRequest(method, path) {
  if (method !== 'GET') return null;
  const pathname = String(path || '').split('?')[0];
  const match = pathname.match(/^\/v1\/models(?:\/([^/]+))?$/);
  if (!match) return null;
  if (!match[1]) return { modelId: null };
  // 非法百分号编码按未命中处理（decodeURIComponent 会抛 URIError）
  try {
    return { modelId: decodeURIComponent(match[1]) };
  } catch (err) {
    return { modelId: match[1] };
  }
}

// 按请求头判定客户端类型（入参为 Node 原生小写键风格的请求头对象）
function resolveClientProtocol(headers) {
  const reqHeaders = headers || {};
  return reqHeaders['anthropic-version'] || reqHeaders['x-api-key'] ? 'anthropic' : 'openai';
}

// 组装对应协议的模型对象
function buildModelEntry(protocolId, id) {
  if (protocolId === 'anthropic') {
    return { type: 'model', id, display_name: id, created_at: new Date().toISOString() };
  }
  return { id, object: 'model', created: Math.floor(Date.now() / 1000), owned_by: 'ccproxy' };
}

// 组装对应协议的 404 响应体
function buildNotFoundPayload(protocolId, modelId) {
  if (protocolId === 'anthropic') {
    return { type: 'error', error: { type: 'not_found_error', message: `model: ${modelId} not found` } };
  }
  return { error: { message: `The model '${modelId}' does not exist`, type: 'invalid_request_error', code: 'model_not_found' } };
}

// 生成响应入口。返回 { status, payload }，由调用方序列化写出
function buildModelsResponse(config, protocolId, modelId) {
  const protocol = (config && Array.isArray(config.protocols) ? config.protocols : []).find((p) => p.id === protocolId);
  if (!protocol) {
    return { status: 404, payload: { error: { type: 'protocol_not_found', message: `配置中不存在协议 ${protocolId}` } } };
  }
  if (!protocol.enabled) {
    return { status: 503, payload: { error: { type: 'protocol_disabled', message: `协议 ${protocol.name}（${protocol.id}）已停用` } } };
  }

  const names = usableClientModels(protocol);

  // 单模型查询：列表里有就返回单项，没有返回协议各自的 404 形态
  if (modelId) {
    if (!names.includes(modelId)) {
      return { status: 404, payload: buildNotFoundPayload(protocolId, modelId) };
    }
    return { status: 200, payload: buildModelEntry(protocolId, modelId) };
  }

  const data = names.map((name) => buildModelEntry(protocolId, name));
  if (protocolId === 'anthropic') {
    return {
      status: 200,
      payload: { data, first_id: data.length ? data[0].id : null, last_id: data.length ? data[data.length - 1].id : null, has_more: false, next_page: null }
    };
  }
  return { status: 200, payload: { object: 'list', data } };
}

module.exports = { matchModelsRequest, resolveClientProtocol, buildModelsResponse, usableClientModels };
