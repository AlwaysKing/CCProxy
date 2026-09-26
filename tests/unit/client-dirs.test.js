// 客户端配置目录单测：身份判定（claude/codex/误判防护）、内置位置、扫描剔除与 kind 矫正
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { builtinDirs, detectDirKind, scanClientDirs } = require('../../electron/services/clients/dirs');

// 每个用例一个临时目录（写在 /tmp 下，不污染项目）
function makeDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ccproxy-dirs-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test('判定：有 settings.json 的是 claude 目录', (t) => {
  const dir = makeDir(t);
  fs.writeFileSync(path.join(dir, 'settings.json'), '{}');
  assert.equal(detectDirKind(dir), 'claude');
});

test('判定：config.toml 带 codex 特征键的是 codex 目录（顶层键与 provider 节两种写法）', (t) => {
  for (const content of ['model_provider = "custom"\n', '[model_providers.custom]\nbase_url = "http://x"\n']) {
    const dir = makeDir(t);
    fs.writeFileSync(path.join(dir, 'config.toml'), content);
    assert.equal(detectDirKind(dir), 'codex');
  }
});

test('判定：config.toml 没有 codex 特征键不算（Rust 的 Cargo 配置就叫这个名）', (t) => {
  const dir = makeDir(t);
  fs.writeFileSync(path.join(dir, 'config.toml'), '[build]\ntarget = "x86_64-apple-darwin"\n');
  assert.equal(detectDirKind(dir), null);
});

test('判定：空目录、不存在的目录、空串都是 null；两个标志都在时按 claude 算', (t) => {
  const dir = makeDir(t);
  assert.equal(detectDirKind(dir), null);
  assert.equal(detectDirKind(path.join(dir, 'no-such')), null);
  assert.equal(detectDirKind(''), null);

  const both = makeDir(t);
  fs.writeFileSync(path.join(both, 'settings.json'), '{}');
  fs.writeFileSync(path.join(both, 'config.toml'), 'model_provider = "custom"\n');
  assert.equal(detectDirKind(both), 'claude');
});

test('内置位置：四个路径、两种身份，home 可注入以便测试', () => {
  const home = '/fake/home';
  assert.deepEqual(builtinDirs({ home }), [
    { kind: 'claude', path: '/fake/home/.claude' },
    { kind: 'codex', path: '/fake/home/.codex' },
    { kind: 'claude', path: '/fake/home/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig' },
    { kind: 'codex', path: '/fake/home/Library/Developer/Xcode/CodingAssistant/codex' }
  ]);
});

test('扫描：内置只收身份对得上的；手动的剔除失效条目并按实测矫正 kind', (t) => {
  const home = makeDir(t);
  // 只造一个有效的内置位置：~/.claude
  fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
  fs.writeFileSync(path.join(home, '.claude', 'settings.json'), '{}');
  // ~/.codex 里放个没有 codex 特征键的 config.toml：路径在、身份不对，不该出现
  fs.mkdirSync(path.join(home, '.codex'), { recursive: true });
  fs.writeFileSync(path.join(home, '.codex', 'config.toml'), '[build]\n');

  const validClaude = makeDir(t);
  fs.writeFileSync(path.join(validClaude, 'settings.json'), '{}');
  const gone = path.join(makeDir(t), 'sub'); // 从不存在的路径
  const kindChanged = makeDir(t);
  fs.writeFileSync(path.join(kindChanged, 'settings.json'), '{}');

  const scan = scanClientDirs({
    home,
    manual: [
      { id: 'd1', kind: 'claude', path: validClaude },
      { id: 'd2', kind: 'codex', path: gone },
      { id: 'd3', kind: 'codex', path: kindChanged }
    ]
  });

  assert.deepEqual(scan.builtin, [{ kind: 'claude', path: path.join(home, '.claude') }]);
  assert.deepEqual(scan.manual, [
    { id: 'd1', kind: 'claude', path: validClaude },
    { id: 'd3', kind: 'claude', path: kindChanged }
  ]);
  assert.deepEqual(scan.dropped, [{ id: 'd2', kind: 'codex', path: gone }]);
});

test('扫描：manual 缺省为空数组', () => {
  const scan = scanClientDirs({ home: '/fake/home' });
  assert.deepEqual(scan.manual, []);
  assert.deepEqual(scan.dropped, []);
});
