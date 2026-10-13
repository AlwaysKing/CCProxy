// 托盘菜单模板：纯函数，不 require electron，便于单测。
// 结构：状态行 → 打开主窗口 / 启停代理 → 退出。
// 没有「切换供应商」：走哪个供应商完全由映射表决定，托盘不设第二个落点，免得两处状态打架

'use strict';

const STATUS_TEXT = {
  running: '运行中',
  stopped: '已停止',
  error: '启动失败'
};

function buildStatusLabel(proxy) {
  if (proxy.status === 'running') return `${STATUS_TEXT.running} · 127.0.0.1:${proxy.port}`;
  const reason = proxy.error ? `：${proxy.error}` : '';
  return `${STATUS_TEXT[proxy.status] || STATUS_TEXT.stopped}${reason}`;
}

// icons = { running, stopped }：托盘那套圆点模板图，由 tray.js 注入（这里不碰 electron）。
// 状态行不再用 ● / ○ 字符——字符是跟菜单文字同色的普通文本，而模板图能与菜单栏图标同形且随深浅色反色
function buildTrayMenu(state, actions, icons = {}) {
  const { proxy } = state;
  return [
    { label: buildStatusLabel(proxy), icon: proxy.status === 'running' ? icons.running : icons.stopped, enabled: false },
    { type: 'separator' },
    { label: '打开主窗口', click: () => actions.openWindow() },
    { label: proxy.status === 'running' ? '停止代理' : '启动代理', click: () => actions.toggleProxy() },
    { type: 'separator' },
    { label: '退出 CCProxy', click: () => actions.quit() }
  ];
}

module.exports = { buildTrayMenu };
