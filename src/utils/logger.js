// 渲染层日志：统一送主进程写文件，同时打到 devtools 控制台
function emit(level, message, data) {
  try {
    window.electronAPI?.log(level, message, data)
  } catch (error) {
    // 日志本身失败不能影响界面
  }
  const suffix = data === undefined ? '' : data
  if (level === 'error') console.error(message, suffix)
  else if (level === 'warn') console.warn(message, suffix)
  else console.log(message, suffix)
}

export const logger = {
  debug: (message, data) => emit('debug', message, data),
  info: (message, data) => emit('info', message, data),
  warn: (message, data) => emit('warn', message, data),
  error: (message, data) => emit('error', message, data)
}
