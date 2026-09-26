// 请求日志存储单测：环形缓冲上限、两份落盘的分目录、用量聚合、清理语义互不影响
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { createRequestLog, dateKey, MAX_ENTRIES } = require('../../electron/storage/request-log');

// 每个用例一个临时目录（写在系统 tmp 下，不污染项目）
function makeLog(t, options = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-log-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return { log: createRequestLog({ dir, ...options }), dir };
}

// 一条正常转发成功的 entry，字段按 server.js 的 makeLogEntry 形态给足
function forwardedEntry(fields) {
  return {
    id: 'x',
    timestamp: Date.now(),
    method: 'POST',
    path: '/v1/messages',
    protocol: 'anthropic',
    clientModel: 'claude-*',
    matchKind: 'wildcard',
    matchedRule: 'claude-*',
    providerId: 'p1',
    providerName: '供应商一',
    realModel: 'glm-4.6',
    statusCode: 200,
    outcome: 'forwarded',
    errorCode: null,
    error: null,
    cancelled: false,
    tokens: { input: 100, output: 20 },
    detection: null,
    ...fields
  };
}

test('内存缓冲留住上限条数且新在旧前', (t) => {
  const { log } = makeLog(t, { maxEntries: 3 });
  for (let i = 0; i < 5; i++) log.record(forwardedEntry({ id: `e${i}` }));
  const list = log.list();
  assert.equal(list.length, 3);
  assert.deepEqual(list.map((e) => e.id), ['e4', 'e3', 'e2']);
  assert.equal(MAX_ENTRIES, 500);
});

test('转发成功与失败都记用量，未转发的请求只进日志', (t) => {
  const { log, dir } = makeLog(t);
  log.record(forwardedEntry());
  log.record(forwardedEntry({ realModel: 'glm-4.6', statusCode: 500, tokens: null }));
  log.record(forwardedEntry({ outcome: 'rejected', statusCode: 404, providerId: null, providerName: null, realModel: null, tokens: null }));
  log.record({ ...forwardedEntry(), outcome: 'detect_failed', protocol: null, providerId: null, providerName: null, realModel: null, tokens: null });

  const usage = log.usage();
  assert.equal(usage.totals.requests, 2);
  assert.equal(usage.totals.failed, 1);
  // 两种落盘目录都存在，且日志目录行数多于用量目录（未转发的两条只落在日志里）
  const logLines = fs.readFileSync(path.join(dir, 'logs', `${dateKey(Date.now())}.jsonl`), 'utf8').trim().split('\n');
  const usageLines = fs.readFileSync(path.join(dir, 'usage', `${dateKey(Date.now())}.jsonl`), 'utf8').trim().split('\n');
  assert.equal(logLines.length, 4);
  assert.equal(usageLines.length, 2);
});

test('聚合按天 / 协议 / 供应商 / 模型分桶，取消不算失败', (t) => {
  const { log } = makeLog(t);
  const now = Date.now();
  log.record(forwardedEntry({ timestamp: now, protocol: 'anthropic', providerName: 'A', realModel: 'glm-4.6', tokens: { input: 100, output: 50 } }));
  log.record(forwardedEntry({ timestamp: now, protocol: 'openai', providerName: 'B', realModel: 'glm-4.6', tokens: { input: 10, output: 5 } }));
  log.record(forwardedEntry({ timestamp: now, protocol: 'openai', providerName: 'B', realModel: 'kimi', statusCode: 200, cancelled: true, tokens: { input: 1, output: 1 } }));

  const usage = log.usage();
  assert.equal(usage.totals.requests, 3);
  assert.equal(usage.totals.failed, 0); // 一条失败都没有
  assert.equal(usage.totals.cancelled, 1); // 取消单独记，不进失败
  assert.equal(usage.totals.input, 111);
  assert.equal(usage.totals.output, 56);

  const anthropic = usage.byProtocol.find((b) => b.key === 'anthropic');
  assert.equal(anthropic.output, 50);
  const providerB = usage.byProvider.find((b) => b.key === 'B');
  assert.equal(providerB.requests, 2);
  // 模型维度把两个协议的 glm-4.6 合在一起：同一个模型走哪个协议都是同一个模型
  const glm = usage.byModel.find((b) => b.key === 'glm-4.6');
  assert.equal(glm.requests, 2);
  assert.equal(glm.input, 110);
  assert.equal(usage.byDay.length, 1);
  assert.equal(usage.byDay[0].key, dateKey(now));
});

test('days=1 只含今天，跨天记录落在各自的日文件里', (t) => {
  const { log, dir } = makeLog(t);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  log.record(forwardedEntry({ timestamp: yesterday.getTime(), tokens: { input: 1, output: 1 } }));
  log.record(forwardedEntry({ timestamp: today.getTime(), tokens: { input: 10, output: 10 } }));

  const names = fs.readdirSync(path.join(dir, 'usage')).filter((n) => n.endsWith('.jsonl')).sort();
  assert.equal(names.length, 2);

  const todayOnly = log.usage({ days: 1 });
  assert.equal(todayOnly.totals.requests, 1);
  assert.equal(todayOnly.totals.input, 10);

  const all = log.usage();
  assert.equal(all.totals.requests, 2);
});

test('用量日文件里的坏行跳过，不丢掉整天', (t) => {
  const { log, dir } = makeLog(t);
  log.record(forwardedEntry({ tokens: { input: 5, output: 5 } }));
  const dayFile = path.join(dir, 'usage', `${dateKey(Date.now())}.jsonl`);
  // 合法 JSON 但缺 timestamp 的行（手工编辑才会有）也要跳过，别聚出 "NaN-NaN-NaN" 桶
  fs.appendFileSync(dayFile, '不是JSON\n{也是坏的\n' + JSON.stringify({ realModel: 'glm-4.6' }) + '\n');
  const usage = log.usage();
  assert.equal(usage.totals.requests, 1);
  assert.equal(usage.totals.input, 5);
});

test('清日志与清用量互不影响：清掉的一方连文件一起删', (t) => {
  const { log, dir } = makeLog(t);
  log.record(forwardedEntry());
  assert.ok(fs.readdirSync(path.join(dir, 'logs')).length > 0);
  assert.ok(fs.readdirSync(path.join(dir, 'usage')).length > 0);

  log.clearLogs();
  assert.equal(log.list().length, 0);
  assert.equal(fs.readdirSync(path.join(dir, 'logs')).length, 0);
  assert.ok(fs.readdirSync(path.join(dir, 'usage')).length > 0); // 用量还在

  log.clearUsage();
  assert.equal(fs.readdirSync(path.join(dir, 'usage')).length, 0);

  // 都清掉之后统计归零而不是报错
  const usage = log.usage();
  assert.equal(usage.totals.requests, 0);
});

test('stats 数出两个目录的文件数与字节数', (t) => {
  const { log } = makeLog(t);
  log.record(forwardedEntry());
  const stats = log.stats();
  assert.equal(stats.logs.files, 1);
  assert.equal(stats.usage.files, 1);
  assert.ok(stats.logs.bytes > 0);
  assert.ok(stats.usage.bytes > 0);
  assert.ok(stats.usage.bytes < stats.logs.bytes); // 用量行字段更少，必然更小
});
