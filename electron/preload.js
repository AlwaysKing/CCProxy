// preload：contextBridge 暴露 electronAPI（请求/响应）与 electronEvents（主进程推送订阅）。
// 订阅统一返回取消订阅函数。

'use strict';

const { contextBridge, ipcRenderer } = require('electron');

// 订阅统一返回取消订阅函数，组件卸载时直接调
function subscribe(channel, callback) {
  const listener = (event, payload) => callback(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

contextBridge.exposeInMainWorld('electronAPI', {
  getAppState: () => ipcRenderer.invoke('get-app-state'),
  getConfig: () => ipcRenderer.invoke('get-config'),
  saveConfig: (config) => ipcRenderer.invoke('save-config', config),
  getProxyStatus: () => ipcRenderer.invoke('get-proxy-status'),
  startProxy: () => ipcRenderer.invoke('start-proxy'),
  stopProxy: () => ipcRenderer.invoke('stop-proxy'),
  restartProxy: () => ipcRenderer.invoke('restart-proxy'),
  fetchProviderModels: (payload) => ipcRenderer.invoke('fetch-provider-models', payload),
  log: (...args) => ipcRenderer.send('log', args),
  platform: process.platform
});

contextBridge.exposeInMainWorld('electronEvents', {
  onConfigChanged: (callback) => subscribe('config-changed', callback),
  onProxyStatusChanged: (callback) => subscribe('proxy-status-changed', callback)
});
