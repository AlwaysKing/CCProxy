// 托盘：图标反映代理运行状态，菜单按协议分组切换当前供应商。
// 配置或状态变化后由 main.cjs 调 refresh() 重建菜单。

'use strict';

const path = require('path');
const { Tray, Menu, nativeImage } = require('electron');
const { buildTrayMenu } = require('./tray-menu');

// resources/ 会打进 app.asar；build/ 是 electron-builder 的构建资源目录，不会被打包
const ICON_DIR = path.join(__dirname, '..', 'resources', 'icons');

let tray = null;
let getState = null;
let actions = null;

// macOS 模板图：黑色 + alpha，系统按深浅色自动反色。@2x 由系统按同名规则自动加载
const ICON_FILES = {
  running: 'trayRunningTemplate.png',
  stopped: 'trayStoppedTemplate.png'
};

// 状态一变就要重建菜单，图片只从磁盘读一次，之后复用
const iconCache = new Map();

function templateIcon(name) {
  if (!iconCache.has(name)) {
    const image = nativeImage.createFromPath(path.join(ICON_DIR, ICON_FILES[name]));
    image.setTemplateImage(true);
    iconCache.set(name, image);
  }
  return iconCache.get(name);
}

// running 之外（stopped / error）都用停止态那枚空心圆
function statusIcon(status) {
  return templateIcon(status === 'running' ? 'running' : 'stopped');
}

function createTray(options) {
  getState = options.getState;
  actions = options.actions;
  tray = new Tray(statusIcon(getState().proxy.status));
  tray.setToolTip('CCProxy');
  refresh();
  return tray;
}

function refresh() {
  if (!tray || tray.isDestroyed()) return;
  const state = getState();
  tray.setImage(statusIcon(state.proxy.status));
  // 菜单状态行的圆点与托盘图标共用同一套模板图，两处形状口径才不会各画一套
  const icons = { running: statusIcon('running'), stopped: statusIcon('stopped') };
  tray.setContextMenu(Menu.buildFromTemplate(buildTrayMenu(state, actions, icons)));
}

function destroy() {
  if (tray && !tray.isDestroyed()) tray.destroy();
  tray = null;
}

module.exports = { createTray, refresh, destroy };
