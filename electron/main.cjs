// CCProxy 主进程：窗口 + 托盘 + IPC 边界 + 代理生命周期。
// 关窗口只隐藏不退出（代理要继续服务），退出统一走托盘菜单或 Cmd+Q。

'use strict';

const path = require('path');
const { app, BrowserWindow, ipcMain, dialog } = require('electron');
const logger = require('./logger');
const { createConfigManager, createConfigId } = require('./storage/config-manager');
const { createRequestLog } = require('./storage/request-log');
const { createLocalProxy } = require('./services/proxy');
const { fetchUpstreamModels } = require('./services/proxy/fetch-models');
const { builtinDirs, detectDirKind, scanClientDirs } = require('./services/clients/dirs');
const { inspectClientConfig } = require('./services/clients/check');
const { applyClientConfig } = require('./services/clients/apply');
const tray = require('./tray');

// 未打包（开发态）一律加载 Vite dev server，改渲染层代码即时热更新；
// 生产走打包后的 dist/index.html。与 CCGUI 的加载决策一致
const DEV_SERVER_URL = 'http://127.0.0.1:5184';
const RENDERER_INDEX = path.join(__dirname, '..', 'dist', 'index.html');

let mainWindow = null;
let isQuitting = false;
let configManager = null;
let requestLog = null;
let config = null;
let configIssues = [];
let configRecovered = false;
let configBackupPath = null;
let proxyStatus = { status: 'stopped', port: null, error: null };
let proxy = null;

function getState() {
  return { config, proxy: proxyStatus };
}

function broadcast(channel, payload) {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send(channel, payload);
  }
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1180,
    height: 780,
    minWidth: 940,
    minHeight: 620,
    title: 'CCProxy',
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: 16, y: 18 },
    // 必须和渲染层的画布色一致（src/styles/global.css 的 body），否则首屏会先闪一下别的颜色
    backgroundColor: '#1D1D20',
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.js')
    }
  });

  if (!app.isPackaged) {
    mainWindow.loadURL(DEV_SERVER_URL);
  } else {
    mainWindow.loadFile(RENDERER_INDEX);
  }

  // 关窗口只隐藏：代理要继续在后台服务
  mainWindow.on('close', (event) => {
    if (isQuitting) return;
    event.preventDefault();
    mainWindow.hide();
  });
  mainWindow.on('closed', () => {
    mainWindow = null;
  });
  return mainWindow;
}

function showWindow() {
  if (!mainWindow) createWindow();
  if (mainWindow.isMinimized()) mainWindow.restore();
  mainWindow.show();
  mainWindow.focus();
}

function setProxyStatus(next) {
  proxyStatus = next;
  tray.refresh();
  broadcast('proxy-status-changed', proxyStatus);
}

async function startProxy() {
  const next = await proxy.start().then(
    (info) => ({ status: 'running', port: info.port, error: null }),
    (error) => ({ status: 'error', port: config.settings.port, error: error.message })
  );
  setProxyStatus(next);
  logger.info('[Proxy] 启动结果', next);
  return next;
}

async function stopProxy() {
  await proxy.stop();
  setProxyStatus({ status: 'stopped', port: config.settings.port, error: null });
  logger.info('[Proxy] 已停止');
  return proxyStatus;
}

async function toggleProxy() {
  return proxyStatus.status === 'running' ? stopProxy() : startProxy();
}

async function restartProxy() {
  await stopProxy();
  return startProxy();
}

// 配置变更的副作用：端口改了要按新端口重启（否则界面写着新端口、实际还监听着旧端口），
// 开机自启要同步给系统。放在这里而不是渲染层——托盘、设置页或以后别处改配置都走同一条路
function applyConfigEffects(previous, next) {
  if (!previous || previous.settings.launchAtLogin !== next.settings.launchAtLogin) {
    try {
      app.setLoginItemSettings({ openAtLogin: next.settings.launchAtLogin === true });
    } catch (error) {
      logger.warn('[App] 同步开机自启失败', { error: error.message });
    }
  }
  const portChanged = !previous || previous.settings.port !== next.settings.port;
  // 只有正在跑才需要重启；停着的话下次启动自然读新端口
  if (portChanged && proxyStatus.status === 'running') {
    logger.info('[Proxy] 端口变更，按新端口重启', { port: next.settings.port });
    restartProxy().catch((error) => logger.error('[Proxy] 端口变更后重启失败', { error: error.message }));
  }
}

// 所有配置变更的唯一出口：落盘 → 更新内存 → 应用副作用 → 刷托盘 → 通知界面
function applyConfig(nextConfig) {
  const previous = config;
  const saved = configManager.save(nextConfig);
  config = saved.config;
  configIssues = saved.issues;
  // 用户主动存过配置后，磁盘上已经是这份新配置，不再有「重建出来的那份」，
  // 重建横幅必须退场，否则后续的校验问题会被错并进「已重建」里
  configRecovered = false;
  configBackupPath = null;
  applyConfigEffects(previous, config);
  tray.refresh();
  broadcast('config-changed', {
    config,
    issues: configIssues,
    recovered: configRecovered,
    backupPath: configBackupPath || ''
  });
  return saved;
}

// ============================================
// App Config IPC Handlers
// ============================================
ipcMain.handle('get-app-state', async () => {
  try {
    return {
      success: true,
      config,
      issues: configIssues,
      recovered: configRecovered,
      backupPath: configBackupPath,
      configPath: configManager.getPath(),
      proxy: proxyStatus
    };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('get-config', async () => {
  try {
    return { success: true, config };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('save-config', async (event, nextConfig) => {
  try {
    const saved = applyConfig(nextConfig);
    return { success: true, config: saved.config, issues: saved.issues };
  } catch (error) {
    logger.error('[Config] 保存失败', { error: error.message });
    return { success: false, error: error.message };
  }
});

// ============================================
// Client Config Dirs IPC Handlers
// ============================================

// 手动目录的增删、扫描剔除、kind 矫正都改的是配置，统一从这条口落盘：
// applyConfig 会顺带广播给界面，设置页不用自己猜配置变成了什么样
function persistClientDirs(nextDirs) {
  const draft = JSON.parse(JSON.stringify(config));
  draft.settings.clientDirs = nextDirs;
  applyConfig(draft);
}

ipcMain.handle('get-client-dirs', async () => {
  try {
    const manual = config.settings.clientDirs;
    const scan = scanClientDirs({ manual });
    // 剔除与矫正都要落盘；扫描保持原顺序，整体序列化比对就够用
    if (JSON.stringify(scan.manual) !== JSON.stringify(manual)) {
      persistClientDirs(scan.manual);
    }
    // 检查结果跟着目录一起返回：界面拿到的每条目录自带「有没有指向 CCProxy」的状态。
    // 期望地址由当前端口与客户端 token 推出，所以端口或 token 一变，下一次取列表就是新结果
    const context = { port: config.settings.port, token: config.settings.clientToken };
    const inspect = (entry) => ({ ...entry, ...inspectClientConfig(entry.path, entry.kind, context) });
    return {
      success: true,
      builtin: scan.builtin.map(inspect),
      manual: scan.manual.map(inspect),
      dropped: scan.dropped,
      expectedAddress: `http://127.0.0.1:${config.settings.port}`
    };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('pick-client-dir', async () => {
  try {
    const result = await dialog.showOpenDialog(mainWindow, {
      title: '选择客户端配置目录',
      properties: ['openDirectory']
    });
    return { success: true, path: result.canceled ? null : result.filePaths[0] ?? null };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('add-client-dir', async (event, dirPath) => {
  try {
    const target = typeof dirPath === 'string' ? path.normalize(dirPath.trim()) : '';
    if (!target) return { success: false, error: '没有选择目录' };
    const kind = detectDirKind(target);
    if (!kind) return { success: false, error: '不是 Claude 或 Codex 的配置目录' };
    // 内置位置是常量，有效时会自己出现在列表里，手动存一份只会重复
    if (builtinDirs().some((entry) => path.normalize(entry.path) === target)) {
      return { success: false, error: '这是内置扫描位置，不用手动添加' };
    }
    if (config.settings.clientDirs.some((entry) => path.normalize(entry.path) === target)) {
      return { success: false, error: '该目录已在列表中' };
    }
    persistClientDirs([...config.settings.clientDirs, { id: createConfigId('d'), kind, path: target }]);
    logger.info('[ClientDirs] 已添加手动目录', { kind, path: target });
    return { success: true, kind };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

// 把 CCProxy 的地址与 token 写进该目录的配置文件。只认扫描出来的目录：
// 渲染层传什么路径就写什么的话，这条口就成了「往任意文件写内容」
ipcMain.handle('apply-client-dir', async (event, dirPath) => {
  try {
    const target = typeof dirPath === 'string' ? path.normalize(dirPath.trim()) : '';
    const scan = scanClientDirs({ manual: config.settings.clientDirs });
    const entry = [...scan.builtin, ...scan.manual].find((item) => path.normalize(item.path) === target);
    if (!entry) return { success: false, error: '该目录不在扫描结果里，请先刷新' };
    const result = applyClientConfig(entry.path, entry.kind, {
      port: config.settings.port,
      token: config.settings.clientToken
    });
    logger.info('[ClientDirs] 已写入 CCProxy 配置', { kind: entry.kind, file: result.file, backupPath: result.backupPath });
    return { success: true, ...result };
  } catch (error) {
    logger.error('[ClientDirs] 写入失败', { error: error.message });
    return { success: false, error: error.message };
  }
});

ipcMain.handle('remove-client-dir', async (event, dirId) => {
  try {
    persistClientDirs(config.settings.clientDirs.filter((entry) => entry.id !== dirId));
    return { success: true };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

// ============================================
// Local Proxy IPC Handlers
// ============================================
ipcMain.handle('get-proxy-status', async () => {
  try {
    return { success: true, ...proxyStatus };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('start-proxy', async () => {
  try {
    const next = await startProxy();
    return { success: next.status === 'running', ...next };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('stop-proxy', async () => {
  try {
    const next = await stopProxy();
    return { success: true, ...next };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('restart-proxy', async () => {
  try {
    const next = await restartProxy();
    return { success: next.status === 'running', ...next };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

// ============================================
// Provider Models Fetch IPC Handler
// ============================================
ipcMain.handle('fetch-provider-models', async (event, payload) => {
  try {
    return await fetchUpstreamModels(payload || {});
  } catch (error) {
    return { ok: false, error: error.message };
  }
});

// ============================================
// Request Log / Usage IPC Handlers
// ============================================
ipcMain.handle('get-logs', async () => {
  try {
    return { success: true, logs: requestLog.list() };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

// 聚合在主进程做：日文件可能有几十天、几万行，全量塞给渲染层再算不合适
ipcMain.handle('get-usage', async (event, range) => {
  try {
    return { success: true, ...requestLog.usage(range || {}) };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('get-storage-stats', async () => {
  try {
    return { success: true, ...requestLog.stats() };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('clear-logs', async () => {
  try {
    requestLog.clearLogs();
    return { success: true };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

ipcMain.handle('clear-usage', async () => {
  try {
    requestLog.clearUsage();
    return { success: true };
  } catch (error) {
    return { success: false, error: error.message };
  }
});

// ============================================
// Renderer Log IPC Handler
// ============================================
ipcMain.on('log', (event, args) => {
  logger.info('[Renderer]', args);
});

async function bootstrap() {
  logger.initialize();

  configManager = createConfigManager();
  const loaded = configManager.load();
  config = loaded.config;
  configIssues = loaded.issues;
  configRecovered = loaded.recovered;
  configBackupPath = loaded.backupPath;
  if (loaded.recovered) {
    logger.warn('[Config] 配置已损坏并重建', { backupPath: loaded.backupPath, reason: loaded.issues.join('；') });
  }
  // 客户端 token 是配置的必需项（检查客户端配置要用，之后校验入站请求也要用）。
  // 在内存里补一个并落盘：不落盘的话每次启动都会换一个新的，客户端那边填过也对不上
  const needsClientToken = !config.settings.clientToken;
  if (needsClientToken) {
    config.settings.clientToken = createConfigId('t');
  }
  // 首启与损坏重建时，拿到的默认配置只在内存里：必须落盘，否则下次启动还是走同一条分支，
  // 界面里显示的「配置文件」也始终不存在
  if (loaded.firstRun || loaded.recovered || needsClientToken) {
    try {
      const saved = configManager.save(config);
      config = saved.config;
      configIssues = [...configIssues, ...saved.issues];
      logger.info('[Config] 已写入配置', { path: configManager.getPath(), firstRun: loaded.firstRun, recovered: loaded.recovered, generatedClientToken: needsClientToken });
    } catch (error) {
      configIssues = [...configIssues, `配置写入失败：${error.message}`];
      logger.error('[Config] 配置写入失败', { path: configManager.getPath(), error: error.message });
    }
  }
  logger.info('[Config] 已加载', { path: configManager.getPath(), port: config.settings.port, issues: configIssues.length });

  requestLog = createRequestLog({ dir: configManager.getDir() });

  // 启动时按配置同步一次副作用：用户可能在系统设置里关掉了自启，而配置里还写着 true
  applyConfigEffects(null, config);

  proxy = createLocalProxy({
    getConfig: () => config,
    // 请求日志由 requestLog 落盘并推给界面；不再往 app.log 里灌一份——
    // 那份日志没有轮转，逐请求写会把它撑爆，而且两处记录早晚对不上
    onLog: (entry) => {
      requestLog.record(entry);
      broadcast('log-appended', entry);
    }
  });

  createWindow();
  tray.createTray({
    getState,
    actions: {
      openWindow: showWindow,
      toggleProxy,
      quit: () => app.quit()
    }
  });

  startProxy();
  app.on('activate', () => showWindow());
}

// 启动阶段抛错（窗口或托盘建不起来）时应用已经没法用了：报错并退出，
// 不留「代理在 listening 但没有窗口也没有托盘」的半初始化状态
function handleStartupError(error) {
  logger.error('[App] 启动失败', { error: error.message, stack: error.stack });
  dialog.showErrorBox('CCProxy 启动失败', error.message);
  app.quit();
}

// 单实例：代理占固定端口，且托盘应用多开会同时出现两个图标
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => showWindow());
  app.whenReady().then(bootstrap).catch(handleStartupError);
}

app.on('window-all-closed', () => {
  // 托盘应用：窗口全关也要让代理继续服务，这里什么都不做
});

app.on('before-quit', () => {
  isQuitting = true;
  tray.destroy();
  if (proxy) proxy.stop().catch(() => {});
});
