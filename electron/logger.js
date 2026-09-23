// 主进程日志：JSON Lines 写 /tmp/ccproxy/app.log，格式与 CCGUI 的 electron/logger.js 同构。
// 日志是旁路：写失败绝不能影响代理主流程。

'use strict';

const fs = require('fs');
const path = require('path');

const DEFAULT_BASE_DIR = '/tmp/ccproxy';
const LOG_FILE_NAME = 'app.log';
const LEVELS = { DEBUG: 'debug', INFO: 'info', WARN: 'warn', ERROR: 'error' };

let baseDir = DEFAULT_BASE_DIR;
let logPath = path.join(baseDir, LOG_FILE_NAME);
let silent = false;

// 切换日志目录（测试用）；silent 时不打控制台
function configure(options = {}) {
  if (options.baseDir) {
    baseDir = options.baseDir;
    logPath = path.join(baseDir, LOG_FILE_NAME);
  }
  if (typeof options.silent === 'boolean') silent = options.silent;
  return { baseDir, logPath };
}

function initialize() {
  try {
    fs.mkdirSync(baseDir, { recursive: true });
    return true;
  } catch (error) {
    return false;
  }
}

function log(level, message, data) {
  const entry = { timestamp: new Date().toISOString(), level, message };
  if (data !== undefined) entry.data = data;
  try {
    fs.mkdirSync(baseDir, { recursive: true });
    fs.appendFileSync(logPath, `${JSON.stringify(entry)}\n`);
  } catch (error) {
    // 日志写不进去也不能影响主流程
  }
  if (!silent) {
    const suffix = data === undefined ? '' : ` ${JSON.stringify(data)}`;
    console.log(`[${entry.timestamp}] ${level.toUpperCase()} ${message}${suffix}`);
  }
  return entry;
}

module.exports = {
  DEBUG: LEVELS.DEBUG,
  INFO: LEVELS.INFO,
  WARN: LEVELS.WARN,
  ERROR: LEVELS.ERROR,
  configure,
  initialize,
  debug: (message, data) => log(LEVELS.DEBUG, message, data),
  info: (message, data) => log(LEVELS.INFO, message, data),
  warn: (message, data) => log(LEVELS.WARN, message, data),
  error: (message, data) => log(LEVELS.ERROR, message, data),
  getPath: () => logPath
};
