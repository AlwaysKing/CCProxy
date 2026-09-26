// 客户端配置目录：内置位置、目录身份判定、扫描。
// 只依赖 fs/os/path，不 require electron，可以脱离主进程单测。

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

// codex 的特征键。config.toml 这名字撞车太多（Rust 的 Cargo 配置就叫这个），
// 必须在内容里看到 codex 特有的键才认，不然手滑加个 Rust 工程会被当成 codex 目录
const CODEX_MARK = /(^|\n)\s*model_provider\s*=|\[model_providers/;

function fileExists(targetPath) {
  try {
    return fs.statSync(targetPath).isFile();
  } catch {
    return false;
  }
}

function isClaudeDir(dirPath) {
  return fileExists(path.join(dirPath, 'settings.json'));
}

function isCodexDir(dirPath) {
  const configPath = path.join(dirPath, 'config.toml');
  if (!fileExists(configPath)) return false;
  try {
    return CODEX_MARK.test(fs.readFileSync(configPath, 'utf8'));
  } catch {
    return false;
  }
}

// 判定目录是哪家的配置目录：settings.json → claude，config.toml（含 codex 特征键）→ codex，
// 都不是返回 null。两个都在的按 claude 算——claude 的标志文件不撞名，先信它
function detectDirKind(dirPath) {
  if (typeof dirPath !== 'string' || !dirPath.trim()) return null;
  if (isClaudeDir(dirPath)) return 'claude';
  if (isCodexDir(dirPath)) return 'codex';
  return null;
}

// 内置扫描位置：前两个是 CLI 默认家目录，后两个是 Xcode 内置 Coding Assistant 的。
// kind 是「预期身份」：路径存在但内容不是这一家的不算数（比如某目录里只是碰巧有个 settings.json）
function builtinDirs({ home } = {}) {
  const base = home || os.homedir();
  return [
    { kind: 'claude', path: path.join(base, '.claude') },
    { kind: 'codex', path: path.join(base, '.codex') },
    { kind: 'claude', path: path.join(base, 'Library', 'Developer', 'Xcode', 'CodingAssistant', 'ClaudeAgentConfig') },
    { kind: 'codex', path: path.join(base, 'Library', 'Developer', 'Xcode', 'CodingAssistant', 'codex') }
  ];
}

// 扫一遍：内置的只收「路径存在且身份与预期一致」的；手动的逐条实测，不再是配置目录的
// 挑出来交给调用方删（内置的是常量，没有可删的条目）。手动条目的 kind 以实测为准——
// 存的时候是 codex、后来目录长成了 claude 的样子，就按 claude 返回，调用方把矫正落回配置
function scanClientDirs({ home, manual = [] } = {}) {
  const builtin = [];
  for (const entry of builtinDirs({ home })) {
    if (detectDirKind(entry.path) === entry.kind) builtin.push(entry);
  }

  const kept = [];
  const dropped = [];
  for (const entry of manual) {
    const kind = detectDirKind(entry.path);
    if (!kind) {
      dropped.push(entry);
      continue;
    }
    kept.push(kind === entry.kind ? entry : { ...entry, kind });
  }
  return { builtin, manual: kept, dropped };
}

module.exports = { builtinDirs, detectDirKind, scanClientDirs };
