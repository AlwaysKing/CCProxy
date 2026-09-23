// 协议适配器：收敛「认证怎么注入」「model 字段改在哪」这类协议差异，转发逻辑里不写死任何协议细节。
// 第一版只有直通适配器（上游协议 = 来源协议）。将来做跨协议转换时，
// 在这里新增「目标协议 != 来源协议」的适配器实现即可，路由与转发层不用动。
// 响应侧对称地做 model 回写：请求把 clientModel 换成 realModel 发往上游，响应把真实模型名
// 换回 clientModel 还给客户端，保证客户端看到的请求/响应模型名一致（Claude Code 等客户端会展示或校验它）。

'use strict';

// 固定剥离清单：无论什么协议，入站请求携带的凭据一律摘掉，保证客户端的假 key 绝不外泄
const STRIP_INBOUND_HEADERS = ['authorization', 'x-api-key', 'api-key', 'x-goog-api-key', 'proxy-authorization'];

// 就地剥离入站凭据（作用于传入对象本身），返回同一对象便于链式调用
function stripInboundCredentials(headers) {
  for (const name of STRIP_INBOUND_HEADERS) {
    delete headers[name];
  }
  return headers;
}

const PROTOCOL_ADAPTERS = {
  anthropic: {
    protocol: 'anthropic',
    // Anthropic 协议规范认证方式：x-api-key 头
    applyAuth(headers, token) {
      headers['x-api-key'] = token;
    },
    rewriteModel(payload, realModel) {
      if (payload && typeof payload === 'object') payload.model = realModel;
      return payload;
    },
    // 流式响应里 model 只出现在 message_start 事件的 message.model 上
    rewriteStreamDataModel(payload, clientModel) {
      if (payload && payload.type === 'message_start' && payload.message && typeof payload.message.model === 'string') {
        payload.message.model = clientModel;
      }
    }
  },
  openai: {
    protocol: 'openai',
    // OpenAI 协议规范认证方式：Authorization Bearer
    applyAuth(headers, token) {
      headers.authorization = `Bearer ${token}`;
    },
    rewriteModel(payload, realModel) {
      if (payload && typeof payload === 'object') payload.model = realModel;
      return payload;
    },
    // 流式响应里每个 chunk 顶层都带 model
    rewriteStreamDataModel(payload, clientModel) {
      if (payload && typeof payload.model === 'string') payload.model = clientModel;
    }
  }
};

function getAdapter(protocolId) {
  const adapter = PROTOCOL_ADAPTERS[protocolId];
  if (!adapter) throw new Error(`未知协议: ${protocolId}`);
  return adapter;
}

// 生成响应回写器：非流式改 JSON 顶层 model，流式逐 SSE 事件改 data 行。
// 非 JSON（如上游错误页）或没有 model 字段的一律原样透传，绝不因为回写失败弄坏响应。
function createResponseRewriter(protocolId, clientModel) {
  const adapter = getAdapter(protocolId);
  return {
    // 非流式：整体一个 JSON，顶层 model 回写
    rewriteBody(buffer) {
      try {
        const payload = JSON.parse(buffer.toString('utf8'));
        if (payload && typeof payload === 'object' && typeof payload.model === 'string' && payload.model !== clientModel) {
          payload.model = clientModel;
          return Buffer.from(JSON.stringify(payload), 'utf8');
        }
      } catch { /* 非 JSON 响应原样透传 */ }
      return buffer;
    },
    // 流式：一个完整 SSE 事件（含结尾空行），只改写 data: 行里的 JSON
    rewriteSseEvent(eventBuffer) {
      const lines = eventBuffer.toString('utf8').split('\n');
      for (let i = 0; i < lines.length; i++) {
        if (!lines[i].startsWith('data:')) continue;
        try {
          const payload = JSON.parse(lines[i].slice(5));
          adapter.rewriteStreamDataModel(payload, clientModel);
          lines[i] = `data: ${JSON.stringify(payload)}`;
        } catch { /* 非 JSON 的 data 行（如 [DONE]）原样保留 */ }
      }
      return Buffer.from(lines.join('\n'), 'utf8');
    }
  };
}

module.exports = { getAdapter, stripInboundCredentials, createResponseRewriter, STRIP_INBOUND_HEADERS };
