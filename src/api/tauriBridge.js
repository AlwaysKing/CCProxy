// Tauri 桥：把原来 Electron preload 暴露的 window.electronAPI / window.electronEvents
// 按同一形状补上，命令名与参数一一对应到 Rust 侧的 #[tauri::command]。
// 这样 stores 与组件一行都不用改。

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

// 订阅统一返回取消订阅函数，组件卸载时直接调
function subscribe(channel, callback) {
  let unlisten = null
  let disposed = false
  listen(channel, (event) => callback(event.payload)).then((fn) => {
    if (disposed) fn()
    else unlisten = fn
  })
  return () => {
    disposed = true
    if (unlisten) unlisten()
  }
}

// 在 Tauri 环境里装桥；纯浏览器打开（npm run dev）时装一份空壳，
// 界面照常渲染（只是所有命令都返回失败），方便单独调界面
export function installTauriBridge() {
  if (!('__TAURI_INTERNALS__' in window)) {
    console.warn('[CCProxy] 不在 Tauri 环境里，界面命令不可用（只能看布局）。完整功能请用 npm run tauri:dev')
    window.electronAPI = new Proxy(
      { platform: 'darwin' },
      {
        get: (target, key) =>
          key in target
            ? target[key]
            : () => Promise.resolve({ success: false, error: '不在 Tauri 环境里，命令不可用' })
      }
    )
    window.electronEvents = {
      onConfigChanged: () => () => {},
      onProxyStatusChanged: () => () => {},
      onLogAppended: () => () => {}
    }
    return false
  }

  window.electronAPI = {
    getAppState: () => invoke('get_app_state'),
    getConfig: () => invoke('get_config'),
    saveConfig: (config) => invoke('save_config', { config }),
    regenerateClientToken: () => invoke('regenerate_client_token'),
    getProxyStatus: () => invoke('get_proxy_status'),
    startProxy: () => invoke('start_proxy'),
    stopProxy: () => invoke('stop_proxy'),
    restartProxy: () => invoke('restart_proxy'),
    fetchProviderModels: (payload) => invoke('fetch_provider_models', { payload }),
    getClientDirs: () => invoke('get_client_dirs'),
    getCodexDaemon: () => invoke('get_codex_daemon'),
    pickClientDir: () => invoke('pick_client_dir'),
    addClientDir: (dirPath) => invoke('add_client_dir', { path: dirPath }),
    applyClientDir: (dirPath) => invoke('apply_client_dir', { path: dirPath }),
    removeClientDir: (dirId) => invoke('remove_client_dir', { id: dirId }),
    getProjectModels: (protocol) => invoke('get_project_models', { protocol }),
    openProjectModelsWindow: (protocol) => invoke('open_project_models_window', { protocol }),
    refreshProjectModels: (protocol) => invoke('refresh_project_models', { protocol }),
    addProjectBindings: (protocol, paths, model) =>
      invoke('add_project_bindings', { protocol, paths, model }),
    setProjectBindingModel: (protocol, id, model) =>
      invoke('set_project_binding_model', { protocol, id, model }),
    removeProjectBinding: (protocol, id) =>
      invoke('remove_project_binding', { protocol, id }),
    getLogs: () => invoke('get_logs'),
    getUsage: (range) => invoke('get_usage', { range }),
    getStorageStats: () => invoke('get_storage_stats'),
    clearLogs: () => invoke('clear_logs'),
    clearUsage: () => invoke('clear_usage'),
    // 日志是旁路：写不进去也不能影响界面
    log: (level, message, data) => invoke('log', { level, message, data }).catch(() => {}),
    platform: 'darwin'
  }

  window.electronEvents = {
    onConfigChanged: (callback) => subscribe('config-changed', callback),
    onProjectModelsChanged: (callback) => subscribe('project-models-changed', callback),
    onProxyStatusChanged: (callback) => subscribe('proxy-status-changed', callback),
    onLogAppended: (callback) => subscribe('log-appended', callback)
  }

  return true
}

