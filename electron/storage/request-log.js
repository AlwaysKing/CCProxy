// 请求日志存储：内存环形缓冲（日志页实时看）+ 两份按天 JSONL 落盘。
// 两份是有意分开的：logs/ 存全字段供事后排查，usage/ 只存聚合用得上的字段。
// 分开的理由是清理语义不同——清日志不该连用量历史一起清掉，反之亦然。
//
// 落盘是旁路：目录建不起来、磁盘写不进去都不能影响代理主流程；读的时候坏行直接跳过，
// 文件可能被手工编辑过，也可能正好写到一半。

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const MAX_ENTRIES = 500;
const DAY_FILE_PATTERN = /^\d{4}-\d{2}-\d{2}\.jsonl$/;

// 按本地日期切文件：用户说的「今天」是本地时间的今天，按 UTC 切会在跨日时错位一格
function dateKey(timestamp) {
  const date = new Date(timestamp);
  const pad = (value) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

// 用量记录只留聚合用得上的字段，也够看单条的消耗
function toUsageRecord(entry) {
  return {
    timestamp: entry.timestamp,
    protocol: entry.protocol,
    providerId: entry.providerId,
    providerName: entry.providerName,
    clientModel: entry.clientModel,
    realModel: entry.realModel,
    status: entry.cancelled ? 'cancelled' : (entry.statusCode >= 200 && entry.statusCode < 300 ? 'ok' : 'failed'),
    input: entry.tokens ? entry.tokens.input : null,
    output: entry.tokens ? entry.tokens.output : null
  };
}

function emptyBucket() {
  return { requests: 0, failed: 0, cancelled: 0, input: 0, output: 0 };
}

// 取消不算失败：用户自己按了 Esc 不该污染上游错误率
function addToBucket(bucket, record) {
  bucket.requests += 1;
  if (record.status === 'cancelled') bucket.cancelled += 1;
  else if (record.status !== 'ok') bucket.failed += 1;
  if (Number.isFinite(record.input)) bucket.input += record.input;
  if (Number.isFinite(record.output)) bucket.output += record.output;
}

function createRequestLog({ dir, maxEntries } = {}) {
  const baseDir = dir || path.join(os.homedir(), '.ccproxy');
  const logsDir = path.join(baseDir, 'logs');
  const usageDir = path.join(baseDir, 'usage');
  const limit = Number.isInteger(maxEntries) && maxEntries > 0 ? maxEntries : MAX_ENTRIES;
  // 新→旧，回收用
  const entries = [];

  function appendLine(target, payload) {
    try {
      fs.mkdirSync(target, { recursive: true });
      fs.appendFileSync(path.join(target, `${dateKey(payload.timestamp)}.jsonl`), `${JSON.stringify(payload)}\n`);
    } catch (error) {
      // 落盘是旁路，写不进去也不能影响代理
    }
  }

  function dayFiles(target) {
    let names;
    try {
      names = fs.readdirSync(target);
    } catch (error) {
      return [];
    }
    return names.filter((name) => DAY_FILE_PATTERN.test(name)).sort();
  }

  function record(entry) {
    entries.unshift(entry);
    if (entries.length > limit) entries.length = limit;
    appendLine(logsDir, entry);
    // 用量只统计真正打到上游的请求：未命中 / 识别失败的请求没有供应商与模型可归属，
    // 它们要看的是日志页（那里能按状态码过滤），用量页要的是上游消耗与上游错误率
    if (entry.outcome === 'forwarded') appendLine(usageDir, toUsageRecord(entry));
    return entry;
  }

  function list() {
    return entries.slice();
  }

  function removeDayFiles(target) {
    for (const name of dayFiles(target)) {
      try {
        fs.unlinkSync(path.join(target, name));
      } catch (error) {
        // 删不掉就算了，下次清理再来
      }
    }
  }

  // 清日志要连内存缓冲一起清：界面上那 500 条也是「日志」，不清掉会看着像没生效
  function clearLogs() {
    entries.length = 0;
    removeDayFiles(logsDir);
  }

  function clearUsage() {
    removeDayFiles(usageDir);
  }

  function dirStats(target) {
    let bytes = 0;
    let files = 0;
    for (const name of dayFiles(target)) {
      try {
        bytes += fs.statSync(path.join(target, name)).size;
        files += 1;
      } catch (error) {
        // 统计拿不到就当没这个文件
      }
    }
    return { files, bytes };
  }

  function stats() {
    return { dir: baseDir, logs: dirStats(logsDir), usage: dirStats(usageDir) };
  }

  // 读日期区间（含端点，YYYY-MM-DD）内的用量记录；不传就是全部
  function readUsageRecords(from, to) {
    const records = [];
    for (const name of dayFiles(usageDir)) {
      const day = name.slice(0, 10);
      if (from && day < from) continue;
      if (to && day > to) continue;
      let text;
      try {
        text = fs.readFileSync(path.join(usageDir, name), 'utf8');
      } catch (error) {
        continue;
      }
      for (const line of text.split('\n')) {
        if (!line.trim()) continue;
        try {
          records.push(JSON.parse(line));
        } catch (error) {
          // 坏行跳过，不因为一行坏了丢掉整天
        }
      }
    }
    return records;
  }

  // 区间两种给法：显式 from/to（YYYY-MM-DD），或只给 days（含今天在内的最近 N 天）。
  // 日期在这里算而不是让界面算：按本地日期切文件的是这里，两边各算一次容易错开一天
  function resolveRange({ from, to, days } = {}) {
    if (from || to) return { from, to };
    const span = Number(days);
    if (!Number.isInteger(span) || span < 1) return { from: undefined, to: undefined };
    const end = new Date();
    const start = new Date();
    start.setDate(start.getDate() - (span - 1));
    return { from: dateKey(start), to: dateKey(end) };
  }

  // 按天 / 供应商 / 模型三个维度一次算齐：切换维度是纯展示，不该再读一遍磁盘。
  // 模型维度不区分协议——同一个模型走哪个协议都该合在一起算
  function usage(options = {}) {
    const { from, to } = resolveRange(options);
    const records = readUsageRecords(from, to);
    const totals = emptyBucket();
    const byDay = new Map();
    const byProtocol = new Map();
    const byProvider = new Map();
    const byModel = new Map();
    for (const record of records) {
      // 手工编辑或异常写入可能留下缺 timestamp 的合法 JSON 行：跳过，
      // 否则会聚出一个 "NaN-NaN-NaN" 的桶，还固定排在按天列表第一位
      if (!Number.isFinite(record.timestamp)) continue;
      const day = dateKey(record.timestamp);
      const protocol = record.protocol || '（未知协议）';
      const provider = record.providerName || '（未知供应商）';
      const model = record.realModel || '（未知模型）';
      if (!byDay.has(day)) byDay.set(day, { key: day, ...emptyBucket() });
      if (!byProtocol.has(protocol)) byProtocol.set(protocol, { key: protocol, ...emptyBucket() });
      if (!byProvider.has(provider)) byProvider.set(provider, { key: provider, ...emptyBucket() });
      if (!byModel.has(model)) byModel.set(model, { key: model, ...emptyBucket() });
      for (const bucket of [totals, byDay.get(day), byProtocol.get(protocol), byProvider.get(provider), byModel.get(model)]) {
        addToBucket(bucket, record);
      }
    }
    return {
      totals,
      // 天倒序（最近的在前），其余按输出 tokens 排——多的在上面
      byDay: [...byDay.values()].sort((a, b) => (a.key < b.key ? 1 : -1)),
      byProtocol: [...byProtocol.values()].sort((a, b) => b.output - a.output || b.requests - a.requests),
      byProvider: [...byProvider.values()].sort((a, b) => b.output - a.output || b.requests - a.requests),
      byModel: [...byModel.values()].sort((a, b) => b.output - a.output || b.requests - a.requests)
    };
  }

  return {
    record,
    list,
    clearLogs,
    clearUsage,
    stats,
    usage,
    getDirs: () => ({ baseDir, logsDir, usageDir })
  };
}

module.exports = { createRequestLog, dateKey, MAX_ENTRIES };
