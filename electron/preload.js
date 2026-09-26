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
  getClientDirs: () => ipcRenderer.invoke('get-client-dirs'),
  pickClientDir: () => ipcRenderer.invoke('pick-client-dir'),
  addClientDir: (dirPath) => ipcRenderer.invoke('add-client-dir', dirPath),
  applyClientDir: (dirPath) => ipcRenderer.invoke('apply-client-dir', dirPath),
  removeClientDir: (dirId) => ipcRenderer.invoke('remove-client-dir', dirId),
  getLogs: () => ipcRenderer.invoke('get-logs'),
  getUsage: (range) => ipcRenderer.invoke('get-usage', range),
  getStorageStats: () => ipcRenderer.invoke('get-storage-stats'),
  clearLogs: () => ipcRenderer.invoke('clear-logs'),
  clearUsage: () => ipcRenderer.invoke('clear-usage'),
  log: (...args) => ipcRenderer.send('log', args),
  platform: process.platform
});

contextBridge.exposeInMainWorld('electronEvents', {
  onConfigChanged: (callback) => subscribe('config-changed', callback),
  onProxyStatusChanged: (callback) => subscribe('proxy-status-changed', callback),
  onLogAppended: (callback) => subscribe('log-appended', callback)
});
