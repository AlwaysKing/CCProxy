// 上游转发：拼 URL、发请求、SSE 边收边写、超时三档、错误映射。
// 默认响应体原样透传（请求体的改写已在上游调用方完成）；传入 responseRewriter 时启用响应回写——
// 非流式整体缓冲后改写并重算 content-length，流式按 SSE 事件边界（\n\n）凑齐一个事件改写一个。
// 本层只管缓冲与边界切分这类管道机械，JSON 与协议语义全部在回写器（adapters.js）里。

'use strict';

const http = require('http');
const https = require('https');
const { URL } = require('url');

// hop-by-hop 头与每请求必须重算的头，不透传
const HOP_BY_HOP_HEADERS = ['connection', 'keep-alive', 'transfer-encoding', 'upgrade', 'te', 'trailer', 'proxy-authenticate', 'proxy-authorization'];
const REQUEST_HEADER_BLOCKLIST = new Set([...HOP_BY_HOP_HEADERS, 'host', 'content-length', 'accept-encoding']);

// 超时三档：流式首字节 / 流式空闲 / 非流式整体
const FIRST_BYTE_TIMEOUT_MS = 60 * 1000;
const STREAM_IDLE_TIMEOUT_MS = 120 * 1000;
const NON_STREAM_TIMEOUT_MS = 600 * 1000;

// 去掉重复的版本段：baseUrl 已以 v1/v1beta 结尾而剩余路径又带同一段时只保留一份
function dropDuplicatedVersionSegment(baseUrl, remainingPath) {
  const baseSegments = baseUrl.replace(/\/+$/, '').split('/').filter(Boolean);
  const baseTail = baseSegments[baseSegments.length - 1] || '';
  const segments = String(remainingPath || '').split('/').filter(Boolean);
  if (baseTail && segments[0] === baseTail && /^v\d+(?:beta\d*)?$/.test(baseTail)) {
    segments.shift();
  }
  return segments.length ? `/${segments.join('/')}` : '';
}

function buildUpstreamUrl(baseUrl, remainingPath) {
  const base = String(baseUrl || '').replace(/\/+$/, '');
  return `${base}${dropDuplicatedVersionSegment(base, remainingPath)}`;
}

// 出站头清洗：剥 blocklist（键按小写比较；Node 原生请求头本身是小写键）
function sanitizeOutboundHeaders(headers) {
  const outbound = {};
  for (const [name, value] of Object.entries(headers || {})) {
    if (REQUEST_HEADER_BLOCKLIST.has(name.toLowerCase())) continue;
    outbound[name] = value;
  }
  return outbound;
}

// 转发一个请求到上游，把上游响应写回 clientRes。
// options: { url, method, headers, body(Buffer), clientRes, isStreaming, onUpstreamResponse, responseRewriter }
//   responseRewriter 可选：{ rewriteBody(Buffer), rewriteSseEvent(Buffer) }，见 adapters.createResponseRewriter
// 返回 Promise<{ statusCode }>，错误时已向客户端写出 502 并 resolve（绝不 reject，调用方无需 catch）
function forwardRequest(options) {
  const { url, method, headers, body, clientRes, isStreaming, onUpstreamResponse, responseRewriter } = options;
  return new Promise((resolve) => {
    const parsed = new URL(url);
    const transport = parsed.protocol === 'https:' ? https : http;
    let settled = false;
    let responseTimer = null;
    let firstByteTimer = null;

    // 统一清理两档计时器；无论成功还是失败都必须清，否则失败路径会泄漏 60s 计时器
    const clearTimers = () => {
      if (responseTimer) clearTimeout(responseTimer);
      if (firstByteTimer) clearTimeout(firstByteTimer);
    };
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimers();
      resolve(result);
    };
    const fail = (err) => {
      if (settled) return;
      if (!clientRes.headersSent) {
        clientRes.writeHead(502, { 'content-type': 'application/json' });
        clientRes.end(JSON.stringify({ error: { type: 'ccproxy_upstream', message: `上游请求失败: ${err.message}` } }));
      } else {
        clientRes.destroy();
      }
      finish({ statusCode: 502, error: err.message });
    };

    const upstreamReq = transport.request(
      {
        hostname: parsed.hostname,
        port: parsed.port || (parsed.protocol === 'https:' ? 443 : 80),
        path: `${parsed.pathname}${parsed.search}`,
        method,
        headers
      },
      (upstreamRes) => {
        clearTimeout(firstByteTimer);
        if (onUpstreamResponse) onUpstreamResponse(upstreamRes);

        // 流式：空闲计时，每收到一块数据就重置
        const resetIdle = () => {
          if (responseTimer) clearTimeout(responseTimer);
          responseTimer = setTimeout(() => {
            upstreamReq.destroy(new Error(`流式响应空闲超过 ${STREAM_IDLE_TIMEOUT_MS}ms`));
          }, STREAM_IDLE_TIMEOUT_MS);
        };

        if (isStreaming) {
          resetIdle();
          // 回写会改变字节长度：剥掉上游（罕见地）给出的 content-length，避免长度对不上导致客户端截断
          const headers = responseRewriter ? { ...upstreamRes.headers } : upstreamRes.headers;
          if (responseRewriter) delete headers['content-length'];
          clientRes.writeHead(upstreamRes.statusCode, headers);
          if (responseRewriter) {
            // 回写模式：Buffer 拼接凑齐完整事件再改写（避免多字节字符被 chunk 边界切断），
            // \n\n 之后的部分是未完成残余，留到下一块或 end 时冲刷
            let pending = null;
            upstreamRes.on('data', (chunk) => {
              resetIdle();
              pending = pending ? Buffer.concat([pending, chunk]) : chunk;
              let idx;
              while ((idx = pending.indexOf('\n\n')) !== -1) {
                const event = pending.subarray(0, idx + 2);
                pending = pending.subarray(idx + 2);
                clientRes.write(responseRewriter.rewriteSseEvent(event));
              }
            });
            upstreamRes.on('end', () => {
              if (pending && pending.length) clientRes.write(responseRewriter.rewriteSseEvent(pending));
              clientRes.end();
              finish({ statusCode: upstreamRes.statusCode });
            });
          } else {
            upstreamRes.on('data', resetIdle);
            upstreamRes.on('data', (chunk) => clientRes.write(chunk));
            upstreamRes.on('end', () => {
              clientRes.end();
              finish({ statusCode: upstreamRes.statusCode });
            });
          }
        } else if (responseRewriter) {
          // 非流式回写模式：整体缓冲 → 改写 → 重算 content-length（改写后长度会变，不能沿用上游的头）
          responseTimer = setTimeout(() => {
            upstreamReq.destroy(new Error(`非流式响应超过 ${NON_STREAM_TIMEOUT_MS}ms`));
          }, NON_STREAM_TIMEOUT_MS);
          const chunks = [];
          upstreamRes.on('data', (chunk) => chunks.push(chunk));
          upstreamRes.on('end', () => {
            const out = responseRewriter.rewriteBody(Buffer.concat(chunks));
            const headers = { ...upstreamRes.headers, 'content-length': String(out.length) };
            delete headers['transfer-encoding'];
            clientRes.writeHead(upstreamRes.statusCode, headers);
            clientRes.end(out);
            finish({ statusCode: upstreamRes.statusCode });
          });
        } else {
          // 非流式直通：头原样写、体边收边写
          responseTimer = setTimeout(() => {
            upstreamReq.destroy(new Error(`非流式响应超过 ${NON_STREAM_TIMEOUT_MS}ms`));
          }, NON_STREAM_TIMEOUT_MS);
          clientRes.writeHead(upstreamRes.statusCode, upstreamRes.headers);
          upstreamRes.on('data', (chunk) => clientRes.write(chunk));
          upstreamRes.on('end', () => {
            clientRes.end();
            finish({ statusCode: upstreamRes.statusCode });
          });
        }

        upstreamRes.on('error', (err) => {
          clientRes.destroy();
          finish({ statusCode: upstreamRes.statusCode, error: err.message });
        });
      }
    );

    firstByteTimer = setTimeout(() => {
      upstreamReq.destroy(new Error(`上游 ${FIRST_BYTE_TIMEOUT_MS}ms 内无响应`));
    }, FIRST_BYTE_TIMEOUT_MS);

    // 客户端提前断开（取消请求）时中止上游请求，避免连接悬挂与上游继续生成计费
    clientRes.on('close', () => {
      if (settled) return;
      upstreamReq.destroy(new Error('客户端提前断开'));
    });

    upstreamReq.on('error', fail);
    if (body) upstreamReq.write(body);
    upstreamReq.end();
  });
}

module.exports = { buildUpstreamUrl, sanitizeOutboundHeaders, forwardRequest, REQUEST_HEADER_BLOCKLIST };
