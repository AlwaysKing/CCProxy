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

// 每条请求一个进程内唯一的 id：日志页要拿它当列表 key，而同一毫秒里进来两条请求
// （并发客户端很常见）时间戳会撞，拿时间戳当 key 会串行
let requestSeq = 0;

// 统一日志 entry：五个分支（模型列表 / 收体失败 / 识别失败 / 查表失败 / 正常转发）字段必须一致，
// 否则日志页每个字段都要按分支判空。这里只铺默认值与时间，具体字段由调用点覆盖。
// outcome 说明这次请求走到哪一步：models（被接管）/ read_failed / detect_failed / rejected / forwarded
function makeLogEntry(req, startedAt, fields) {
  requestSeq += 1;
  return {
    id: `${startedAt}-${requestSeq}`,
    timestamp: startedAt,
    durationMs: Date.now() - startedAt,
    method: req.method,
    path: req.url,
    protocol: null,
    clientModel: null,
    matchKind: null,
    matchedRule: null,
    providerId: null,
    providerName: null,
    realModel: null,
    statusCode: 0,
    outcome: 'unknown',
    errorCode: null,
    error: null,
    cancelled: false,
    tokens: null,
    detection: null,
    ...fields
  };
}

// 单个请求的完整处理链
async function handleRequest(req, res, options) {
  const { getConfig, onLog, maxBodyBytes } = options;
  const startedAt = Date.now();

  // 收体失败（超 64MB 上限、连接中断）也要留下日志 entry：否则这类请求在日志页凭空消失，
  // 用户只看到 413 却查不到当时发生了什么
  let body;
  try {
    body = await readRequestBody(req, maxBodyBytes);
  } catch (err) {
    const statusCode = err.statusCode || 500;
    sendJsonError(res, statusCode, { error: { type: 'ccproxy_body', message: err.message } });
    if (onLog) {
      onLog(makeLogEntry(req, startedAt, { statusCode, outcome: 'read_failed', errorCode: statusCode === 413 ? 'body_too_large' : 'body_read_error', error: err.message }));
    }
    return;
  }

  // 模型列表接口由代理接管：不透传上游，按当前激活的映射直接生成响应
  const modelsReq = matchModelsRequest(req.method, req.url);
  if (modelsReq) {
    const protocolId = resolveClientProtocol(req.headers);
    const { status, payload } = buildModelsResponse(getConfig(), protocolId, modelsReq.modelId);
    sendJson(res, status, payload);
    if (onLog) onLog(makeLogEntry(req, startedAt, { protocol: protocolId, clientModel: modelsReq.modelId, statusCode: status, outcome: 'models' }));
    return;
  }

  const detection = detectProtocol({ method: req.method, path: req.url, headers: req.headers, body });
  if (!detection.protocol) {
    sendJsonError(res, 400, {
      error: { type: 'ccproxy_detect', message: '无法识别请求所属协议', detail: { scores: detection.scores, reasons: detection.reasons } }
    });
    if (onLog) onLog(makeLogEntry(req, startedAt, { statusCode: 400, outcome: 'detect_failed', detection }));
    return;
  }

  const payload = parseBody(body);
  const clientModel = payload && typeof payload.model === 'string' ? payload.model : '';
  const config = getConfig();
  const routing = resolveRouting(config, detection.protocol, clientModel);
  if (routing.error) {
    sendJsonError(res, routing.error.status, { error: { type: routing.error.code, message: routing.error.message } });
    if (onLog) onLog(makeLogEntry(req, startedAt, { protocol: detection.protocol, clientModel, statusCode: routing.error.status, outcome: 'rejected', errorCode: routing.error.code, detection }));
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

  // 用量在回写器里攒着（它按协议语义解析过响应体），到这里取一次；
  // 上游没上报就是 null，与「上报了 0」区分开
  if (onLog) {
    onLog(makeLogEntry(req, startedAt, {
      protocol: detection.protocol,
      clientModel,
      matchKind: routing.matchKind,
      matchedRule: routing.mapping.clientModel,
      mappingId: routing.mapping.id,
      providerId: provider.id,
      providerName: provider.name,
      realModel: routing.realModel,
      statusCode: result.statusCode,
      outcome: 'forwarded',
      cancelled: result.cancelled === true,
      error: result.error || null,
      tokens: responseRewriter ? responseRewriter.getUsage() : null,
      detection
    }));
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
