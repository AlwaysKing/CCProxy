// 供应商模型拉取：按协议的认证方式请求上游 GET /v1/models，解析出模型 id 列表。
// 供供应商编辑弹窗「拉取」按钮使用；走主进程 Node 请求，绕开渲染进程的 CORS 限制。
// 与 forwardRequest 同一契约风格：绝不 reject，成败都 resolve { ok, ... }。

'use strict';

const http = require('http');
const https = require('https');
const { URL } = require('url');
const { buildUpstreamUrl } = require('./upstream');
const { getAdapter } = require('./adapters');

const FETCH_TIMEOUT_MS = 10 * 1000;

// 拉取上游模型列表。入参 { protocolId, baseUrl, token }；baseUrl 与弹窗里填的一致，
// 是否带 /v1 由 buildUpstreamUrl 统一处理（重复版本段自动去重）。
function fetchUpstreamModels({ protocolId, baseUrl, token }) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      resolve(result);
    };

    // 头组装单独 try：protocolId 未知时 getAdapter 同步抛异常，须转成 { ok: false }
    const headers = { accept: 'application/json' };
    try {
      if (token) getAdapter(protocolId).applyAuth(headers, token);
      if (protocolId === 'anthropic') headers['anthropic-version'] = '2023-06-01';
    } catch (err) {
      finish({ ok: false, error: err.message });
      return;
    }

    let req;
    try {
      const url = buildUpstreamUrl(baseUrl, '/v1/models');
      const parsed = new URL(url);
      req = (parsed.protocol === 'https:' ? https : http).get(url, { headers }, (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          if (res.statusCode < 200 || res.statusCode >= 300) {
            finish({ ok: false, error: `上游返回 HTTP ${res.statusCode}` });
            return;
          }
          // 两种协议的模型列表响应结构一致：顶层 data 数组、条目带 id
          let payload = null;
          try {
            payload = JSON.parse(Buffer.concat(chunks).toString('utf8'));
          } catch (err) {
            payload = null;
          }
          const ids = Array.isArray(payload && payload.data)
            ? payload.data.map((m) => (m && typeof m.id === 'string' ? m.id.trim() : '')).filter(Boolean)
            : null;
          if (!ids) {
            finish({ ok: false, error: '上游响应里没有可识别的模型列表' });
            return;
          }
          finish({ ok: true, models: [...new Set(ids)] });
        });
      });
    } catch (err) {
      finish({ ok: false, error: `上游地址无效：${err.message}` });
      return;
    }
    req.on('error', (err) => finish({ ok: false, error: err.message }));
    req.setTimeout(FETCH_TIMEOUT_MS, () => req.destroy(new Error(`拉取超时（${FETCH_TIMEOUT_MS / 1000} 秒）`)));
  });
}

module.exports = { fetchUpstreamModels };
