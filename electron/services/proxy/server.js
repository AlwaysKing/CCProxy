// 本地代理服务：单端口接收所有协议请求，识别协议 → 查映射表 → 协议适配改写 → 转发上游。
// 配置通过注入的 getConfig() 每请求现读，改配置即时生效，无需重启代理。

'use strict';

const http = require('http');
const { detectProtocol, parseBody } = require('./detector');
const { resolveRouting } = require('./router');
const { matchModelsRequest, resolveClientProtocol, buildModelsResponse } = require('./models');
const { getAdapter, stripInboundCredentials, createResponseRewriter } = require('./adapters');
const { buildUpstreamUrl, sanitizeOutboundHeaders, forwardRequest } = require('./upstream');

const MAX_BODY_BYTES = 64 * 1024 * 1024;

// 读完整请求体；超过上限报 413。注意：只 pause 不 destroy——先让上层把 413 响应
// 写出去，剩余请求体由 Node 丢弃（destroy 会连 socket 一起销毁，客户端就只能收到连接重置）
function readRequestBody(req, maxBodyBytes) {
  const limit = maxBodyBytes || MAX_BODY_BYTES;
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    let overLimit = false;
    req.on('data', (chunk) => {
      if (overLimit) return;
      size += chunk.length;
      if (size > limit) {
        overLimit = true;
        reject(Object.assign(new Error(`请求体超过 ${limit} 字节上限`), { statusCode: 413 }));
        req.pause();
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => {
      if (!overLimit) resolve(Buffer.concat(chunks));
    });
    req.on('error', (err) => {
      if (!overLimit) reject(err);
    });
  });
}

// 统一的 JSON 响应
function sendJson(res, statusCode, payload) {
  if (res.headersSent) {
    res.destroy();
    return;
  }
  res.writeHead(statusCode, { 'content-type': 'application/json' });
  res.end(JSON.stringify(payload));
}

// 统一的 JSON 错误响应
function sendJsonError(res, statusCode, payload) {
  sendJson(res, statusCode, payload);
}

// 单个请求的完整处理链
async function handleRequest(req, res, options) {
  const { getConfig, onLog, maxBodyBytes } = options;
  const startedAt = Date.now();
  const body = await readRequestBody(req, maxBodyBytes);

  // 模型列表接口由代理接管：不透传上游，按当前激活的映射直接生成响应
  const modelsReq = matchModelsRequest(req.method, req.url);
  if (modelsReq) {
    const protocolId = resolveClientProtocol(req.headers);
    const { status, payload } = buildModelsResponse(getConfig(), protocolId, modelsReq.modelId);
    sendJson(res, status, payload);
    if (onLog) onLog({ timestamp: startedAt, protocol: protocolId, clientModel: modelsReq.modelId, statusCode: status, durationMs: Date.now() - startedAt, handled: 'models' });
    return;
  }

  const detection = detectProtocol({ method: req.method, path: req.url, headers: req.headers, body });
  if (!detection.protocol) {
    sendJsonError(res, 400, {
      error: { type: 'ccproxy_detect', message: '无法识别请求所属协议', detail: { scores: detection.scores, reasons: detection.reasons } }
    });
    if (onLog) onLog({ timestamp: startedAt, protocol: null, clientModel: null, statusCode: 400, durationMs: Date.now() - startedAt, detection });
    return;
  }

  const payload = parseBody(body);
  const clientModel = payload && typeof payload.model === 'string' ? payload.model : '';
  const config = getConfig();
  const routing = resolveRouting(config, detection.protocol, clientModel);
  if (routing.error) {
    sendJsonError(res, routing.error.status, { error: { type: routing.error.code, message: routing.error.message } });
    if (onLog) onLog({ timestamp: startedAt, protocol: detection.protocol, clientModel, statusCode: routing.error.status, durationMs: Date.now() - startedAt, detection });
    return;
  }

  const adapter = getAdapter(detection.protocol);
  const provider = routing.provider;

  // 改写 model 字段；只有真的变了才重新序列化，没变就原样字节透传
  let forwardBody = body;
  if (payload && payload.model !== routing.realModel) {
    adapter.rewriteModel(payload, routing.realModel);
    forwardBody = Buffer.from(JSON.stringify(payload), 'utf8');
  }

  // 出站头：清洗 → 剥离入站凭据 → 注入供应商真实凭据 → 重算 content-length
  const outboundHeaders = stripInboundCredentials(sanitizeOutboundHeaders(req.headers));
  adapter.applyAuth(outboundHeaders, provider.token);
  outboundHeaders['content-length'] = String(forwardBody.length);

  const isStreaming = !!(payload && payload.stream === true) || String(req.headers.accept || '').includes('text/event-stream');
  const url = buildUpstreamUrl(provider.baseUrl, req.url);

  // 响应回写：把上游回传的真实模型名换回 clientModel，保证请求/响应两侧模型名一致。
  // 请求体里连 model 都没有时不回写，避免把响应里的 model 改成空串
  const responseRewriter = clientModel ? createResponseRewriter(detection.protocol, clientModel) : null;

  const result = await forwardRequest({
    url,
    method: req.method,
    headers: outboundHeaders,
    body: forwardBody,
    clientRes: res,
    isStreaming,
    onUpstreamResponse: () => {},
    responseRewriter
  });

  if (onLog) {
    onLog({
      timestamp: startedAt,
      protocol: detection.protocol,
      clientModel,
      mappingId: routing.mapping.id,
      providerName: provider.name,
      realModel: routing.realModel,
      statusCode: result.statusCode,
      durationMs: Date.now() - startedAt,
      detection
    });
  }
}

function createProxyServer(options = {}) {
  const handleOptions = { getConfig: options.getConfig, onLog: options.onLog, maxBodyBytes: options.maxBodyBytes };
  return http.createServer((req, res) => {
    handleRequest(req, res, handleOptions).catch((err) => {
      sendJsonError(res, err.statusCode || 500, { error: { type: 'ccproxy_internal', message: err.message } });
    });
  });
}

module.exports = { createProxyServer, readRequestBody, sendJson, sendJsonError };
