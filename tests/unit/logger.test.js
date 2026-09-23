// 日志单测：JSON Lines 格式、目录可切换、未初始化也不抛错
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const logger = require('../../electron/logger');

function makeDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-log-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test('初始化后写入的一行是合法 JSON 且含 level/message/data', (t) => {
  const dir = makeDir(t);
  logger.configure({ baseDir: dir, silent: true });
  logger.initialize();
  logger.info('[Proxy] 测试', { port: 18888 });

  const lines = fs.readFileSync(logger.getPath(), 'utf8').trim().split('\n');
  assert.equal(lines.length, 1);
  const entry = JSON.parse(lines[0]);
  assert.equal(entry.level, 'info');
  assert.equal(entry.message, '[Proxy] 测试');
  assert.deepEqual(entry.data, { port: 18888 });
  assert.ok(Date.parse(entry.timestamp) > 0);
});

test('未初始化也能写，目录自动创建且不抛错', (t) => {
  const dir = path.join(makeDir(t), 'nested');
  logger.configure({ baseDir: dir, silent: true });
  assert.doesNotThrow(() => logger.warn('未初始化'));
  assert.ok(fs.existsSync(logger.getPath()));
});

test('configure 切目录后写到新目录', (t) => {
  const first = makeDir(t);
  const second = makeDir(t);
  logger.configure({ baseDir: first, silent: true });
  logger.error('第一条');
  logger.configure({ baseDir: second, silent: true });
  logger.error('第二条');
  assert.match(fs.readFileSync(path.join(first, 'app.log'), 'utf8'), /第一条/);
  assert.match(fs.readFileSync(path.join(second, 'app.log'), 'utf8'), /第二条/);
});

test('data 为空时不写 data 字段', (t) => {
  const dir = makeDir(t);
  logger.configure({ baseDir: dir, silent: true });
  logger.error('无数据');
  const entry = JSON.parse(fs.readFileSync(logger.getPath(), 'utf8').trim().split('\n')[0]);
  assert.equal('data' in entry, false);
});
