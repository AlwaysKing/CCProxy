// 托盘：图标反映代理运行状态，菜单按协议分组切换当前供应商。
// 配置或状态变化后由 main.cjs 调 refresh() 重建菜单。

'use strict';

const path = require('path');
const { Tray, Menu, nativeImage } = require('electron');
const { buildTrayMenu } = require('./tray-menu');

const ICON_DIR = path.join(__dirname, '..', 'build', 'icons');

let tray = null;
let getState = null;
let actions = null;

// macOS 模板图：黑色 + alpha，系统按深浅色自动反色。@2x 由系统按同名规则自动加载
function iconFor(status) {
  const fileName = status === 'running' ? 'trayRunningTemplate.png' : 'trayStoppedTemplate.png';
  const image = nativeImage.createFromPath(path.join(ICON_DIR, fileName));
  image.setTemplateImage(true);
  return image;
}

function createTray(options) {
  getState = options.getState;
  actions = options.actions;
  tray = new Tray(iconFor(getState().proxy.status));
  tray.setToolTip('CCProxy');
  refresh();
  return tray;
}

function refresh() {
  if (!tray || tray.isDestroyed()) return;
  const state = getState();
  tray.setImage(iconFor(state.proxy.status));
  tray.setContextMenu(Menu.buildFromTemplate(buildTrayMenu(state, actions)));
}

function destroy() {
  if (tray && !tray.isDestroyed()) tray.destroy();
  tray = null;
}

module.exports = { createTray, refresh, destroy };
