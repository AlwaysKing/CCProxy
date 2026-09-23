// 协议识别器：根据请求路径、请求头、请求体特征判断请求属于哪种 agent 协议。
// 打分制：anthropic 与 openai 各自累计得分，高分者胜；并列或全零视为无法识别。
// 无法识别时返回 protocol = null，并把各项依据带回去供错误响应与日志使用。

'use strict';

// 路径规则：命中即给该协议加分。分数刻意给得比头/体特征高——路径是最强信号
const PATH_RULES = [
  { protocol: 'anthropic', score: 3, pattern: /^\/v1\/messages(?:\/|$)/ },
  { protocol: 'openai', score: 3, pattern: /^\/v1\/(?:chat\/completions|responses|completions|embeddings|models)(?:\/|$)/ }
];

// 解析请求体为 JSON 对象；失败或非对象返回 null，绝不抛异常
function parseBody(body) {
  if (!body) return null;
  try {
    const parsed = Buffer.isBuffer(body) ? JSON.parse(body.toString('utf8')) : JSON.parse(body);
    return parsed && typeof parsed === 'object' ? parsed : null;
  } catch (err) {
    return null;
  }
}

// 识别入口。入参 headers 需为 Node 原生小写键风格的请求头对象
function detectProtocol({ method, path, headers, body }) {
  const scores = { anthropic: 0, openai: 0 };
  const reasons = [];
  const pathname = String(path || '').split('?')[0];
  const reqHeaders = headers || {};

  for (const rule of PATH_RULES) {
    if (rule.pattern.test(pathname)) {
      scores[rule.protocol] += rule.score;
      reasons.push(`路径 ${pathname} 命中 ${rule.protocol} 路径规则 +${rule.score}`);
    }
  }
  if (reqHeaders['anthropic-version']) {
    scores.anthropic += 3;
    reasons.push('请求头 anthropic-version +3');
  }
  if (reqHeaders['x-api-key']) {
    scores.anthropic += 1;
    reasons.push('请求头 x-api-key +1');
  }
  const payload = parseBody(body);
  if (payload && 'system' in payload && 'max_tokens' in payload) {
    scores.anthropic += 2;
    reasons.push('请求体顶层同时含 system 与 max_tokens +2');
  }

  let protocol = null;
  if (scores.anthropic !== scores.openai) {
    protocol = scores.anthropic > scores.openai ? 'anthropic' : 'openai';
  }
  return { protocol, scores, reasons };
}

module.exports = { detectProtocol, parseBody };
