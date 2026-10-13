// CCProxy 主进程：窗口 + 托盘 + IPC 边界 + 代理生命周期。
// 关窗口只隐藏不退出（代理要继续服务），退出统一走托盘菜单或 Cmd+Q。

'use strict';

const path = require('path');
const { app, BrowserWindow, ipcMain, dialog } = require('electron');
const logger = require('./logger');
const { createConfigManager, createConfigId, createClientToken } = require('./storage/config-manager');
const { createRequestLog } = require('./storage/request-log');
const { createLocalProxy } = require('./services/proxy');
const { fetchUpstreamModels } = require('./services/proxy/fetch-models');
const { builtinDirs, detectDirKind, scanClientDirs } = require('./services/clients/dirs');
const { inspectClientConfig } = require('./services/clients/check');
const { applyClientConfig, syncCodexCatalogs } = require('./services/clients/apply');
const { expectedCatalogEntries } = require('./services/clients/catalog');
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

  // 关窗口只隐藏：代理要继续在后台服务。同时隐藏 Dock 图标（agent 应用模式），
  // 看起来像已退出，实际进程和托盘还活着；showWindow() 时再恢复
  mainWindow.on('close', (event) => {
    if (isQuitting) return;
    event.preventDefault();
    mainWindow.hide();
    if (process.platform === 'darwin' && app.dock) app.dock.hide();
  });
  mainWindow.on('closed', () => {
    mainWindow = null;
  });
  return mainWindow;
}

function showWindow() {
  if (!mainWindow) createWindow();
  if (mainWindow.isMinimized()) mainWindow.restore();
  // 恢复 Dock 图标（关窗口时被隐藏了）再显示窗口，否则窗口没有所属的 Dock 入口
  if (process.platform === 'darwin' && app.dock) app.dock.show();
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
  // 协议配置一变，已托管 codex 目录的模型清单就过期了——趁广播通知界面之前刷掉，
  // 界面随后重扫时看到的就是新结果，不会闪一个「不一致」出来
  const protocolsChanged = !previous || JSON.stringify(previous.protocols) !== JSON.stringify(next.protocols);
  if (protocolsChanged) syncManagedCodexCatalogs();
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

// 重新生成客户端 token：与 persistClientDirs 一样走 applyConfig 落盘并广播。
// 代理按请求读最新配置，不用重启。原本就配好（地址与 token 都对）的目录属于「被换 token 连坐」，
// 自动跟着写入新值，不让用户白点一遍「应用」；原本就不一致的本来就要人工处理，不趁机动它——
// 所以 ok 名单必须按旧 token 先扫出来，换完再扫就全不一致了。
// 返回新配置让界面直接回填，不赌广播与响应的到达顺序
ipcMain.handle('regenerate-client-token', async () => {
  try {
    const before = scanClientDirs({ manual: config.settings.clientDirs });
    const wasOk = [...before.builtin, ...before.manual].filter((entry) => entry.status === 'ok');

    const draft = JSON.parse(JSON.stringify(config));
    draft.settings.clientToken = createClientToken();
    applyConfig(draft);
    logger.info('[Config] 已重新生成客户端 token');

    const reapplied = [];
    const errors = [];
    for (const entry of wasOk) {
      try {
        const result = applyClientConfig(entry.path, entry.kind, {
          port: config.settings.port,
          token: config.settings.clientToken,
          ...codexCatalogContext()
        });
        reapplied.push(entry.path);
        logger.info('[ClientDirs] 换 token 后已自动写入', { kind: entry.kind, path: entry.path, files: result.map((item) => item.file) });
      } catch (error) {
        errors.push({ path: entry.path, message: error.message });
        logger.error('[ClientDirs] 换 token 后自动写入失败', { path: entry.path, error: error.message });
      }
    }
    return { success: true, config, reapplied, errors };
  } catch (error) {
    logger.error('[Config] 重新生成客户端 token 失败', { error: error.message });
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

// codex 模型清单的期望条目：OpenAI 协议下启用的映射 → 名字用 clientModel、能力取映射
// 指向的真实模型。检查与应用共用这一份，两边口径才不会漂
function codexCatalogContext() {
  const protocol = (config.protocols || []).find((item) => item.id === 'openai');
  return { catalog: expectedCatalogEntries(protocol) };
}

// 协议配置变了（模型能力、映射、供应商启停）就把已归我们管的 codex 目录的清单刷一遍，
// 用户改完配置不用再跑去设置页点「应用」。没归我们管的目录不动，留给手动应用。
// 失败只记日志不打断：清单没刷成，下一次目录检查会把不一致报出来
function syncManagedCodexCatalogs() {
  try {
    const scan = scanClientDirs({ manual: config.settings.clientDirs });
    const context = { port: config.settings.port, token: config.settings.clientToken, ...codexCatalogContext() };
    const { applied, errors } = syncCodexCatalogs([...scan.builtin, ...scan.manual], context);
    if (applied.length) logger.info('[ClientDirs] 模型清单已随配置自动更新', { files: applied.map((item) => item.file) });
    for (const item of errors) logger.error('[ClientDirs] 模型清单自动更新失败', { path: item.path, error: item.message });
  } catch (error) {
    logger.error('[ClientDirs] 模型清单自动更新失败', { error: error.message });
  }
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
    // 期望地址由当前端口与客户端 token 推出，所以端口或 token 一变，下一次取列表就是新结果；
    // codex 还多带一份模型清单的期望条目，映射改了重扫结论也会跟着变
    const context = { port: config.settings.port, token: config.settings.clientToken, ...codexCatalogContext() };
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
      token: config.settings.clientToken,
      ...codexCatalogContext()
    });
    // result 是写入的文件清单（config.toml 之外可能还有 models.json），逐个记上
    const written = result.map((item) => item.file);
    logger.info('[ClientDirs] 已写入 CCProxy 配置', { kind: entry.kind, files: written });
    return { success: true, files: result };
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

  // 开发模式下 Dock 显示的是默认 Electron 图标，这里手动换成应用图标（打包后的 app 会用 icns，不受影响）
  if (process.platform === 'darwin' && app.dock) {
    app.dock.setIcon(path.join(__dirname, '..', 'resources', 'icons', 'icon.png'));
  }

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
    config.settings.clientToken = createClientToken();
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
