// 托盘菜单模板单测：状态行文案、启停项随状态翻转
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { buildTrayMenu } = require('../../electron/tray-menu');

const baseActions = {
  openWindow() {},
  toggleProxy() {},
  quit() {}
};

function state(proxy) {
  return { proxy, config: { protocols: [] } };
}

test('运行中：状态行含端口，启停项为停止代理', () => {
  const menu = buildTrayMenu(state({ status: 'running', port: 18888, error: null }), baseActions);
  assert.match(menu[0].label, /18888/);
  assert.equal(menu[0].enabled, false);
  assert.ok(menu.some((item) => item.label === '停止代理'));
});

test('启动失败：状态行含错误信息，启停项为启动代理', () => {
  const menu = buildTrayMenu(state({ status: 'error', port: 18888, error: 'EADDRINUSE' }), baseActions);
  assert.match(menu[0].label, /EADDRINUSE/);
  assert.ok(menu.some((item) => item.label === '启动代理'));
});

// 走哪个供应商只看映射表，托盘里不该再有一套「当前启用」的影子状态
test('菜单里没有供应商切换项', () => {
  const menu = buildTrayMenu(state({ status: 'stopped', port: 18888, error: null }), baseActions);
  assert.equal(menu.some((item) => item.type === 'radio' || item.submenu), false);
  assert.deepEqual(
    menu.map((item) => item.label).filter(Boolean),
    ['已停止', '打开主窗口', '启动代理', '退出 CCProxy']
  );
});

// 状态行靠图标图片表状态（注入自 tray.js），不再用 ● / ○ 字符
test('状态行挂对应状态的图标，文案里不再有 ● / ○', () => {
  const icons = { running: { name: 'running' }, stopped: { name: 'stopped' } };

  const running = buildTrayMenu(state({ status: 'running', port: 18888, error: null }), baseActions, icons);
  assert.equal(running[0].icon, icons.running);
  assert.equal(running[0].label, '运行中 · 127.0.0.1:18888');

  // 启动失败也归到停止态那枚图
  const failed = buildTrayMenu(state({ status: 'error', port: 18888, error: 'EADDRINUSE' }), baseActions, icons);
  assert.equal(failed[0].icon, icons.stopped);
  assert.equal(failed[0].label, '启动失败：EADDRINUSE');

  for (const item of [...running, ...failed]) {
    assert.doesNotMatch(item.label || '', /[●○]/);
  }
});
