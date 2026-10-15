// 展示层格式化：日志页与用量页共用。都放这里而不是各自写一份——
// 「多久算多久」这类口径两边不一致时，同一个数在两页看起来会像两个数

function pad(value) {
  return String(value).padStart(2, '0')
}

// 日志页一行一条，只看几点几分几秒；日期放详情里，列表里天天都一样没必要重复
export function formatClock(timestamp) {
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return '—'
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

export function formatDateTime(timestamp) {
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return '—'
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${formatClock(timestamp)}`
}

// 耗时：秒以上换成带单位的写法，毫秒级直接给整数；不足 1ms 也显示 1ms——
// 显示 0ms 会让人以为没计时。分秒的拆法是先整到秒再拆：先取模再取整会在分钟边界
// 凑出「1m60s」
export function formatDuration(ms) {
  const value = Number(ms)
  if (!Number.isFinite(value) || value < 0) return '—'
  if (value < 1000) return `${Math.max(1, Math.round(value))}ms`
  if (value < 60000) return `${(value / 1000).toFixed(1)}s`
  const totalSeconds = Math.round(value / 1000)
  return `${Math.floor(totalSeconds / 60)}m${totalSeconds % 60}s`
}

export function formatNumber(value) {
  // null/undefined 是「上游没上报」，与 0 不是一回事：直接当 0 显示会让人以为真没有消耗
  if (value === null || value === undefined) return '—'
  const number = Number(value)
  if (!Number.isFinite(number)) return '—'
  return number.toLocaleString('zh-CN')
}

// tokens 动辄六位数，横向空间有限；未上报（null/undefined）要显式说清楚，
// 跟「上报了 0」不是一回事
export function formatTokens(value) {
  if (value === null || value === undefined) return '—'
  const number = Number(value)
  if (!Number.isFinite(number)) return '—'
  if (number < 10000) return number.toLocaleString('zh-CN')
  // 分档看的是「四舍五入之后」的量级：999,950 除以千是 999.95，toFixed(1) 会凑成 1000.0k，
  // 这种写法既没意义又比 M 长，所以从它开始就进位到 M
  if (number < 999950) return `${(number / 1000).toFixed(1)}k`
  return `${(number / 1000000).toFixed(2)}M`
}

export function formatBytes(bytes) {
  const value = Number(bytes)
  if (!Number.isFinite(value) || value < 0) return '—'
  if (value < 1024) return `${value}B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)}KB`
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)}MB`
  return `${(value / 1024 / 1024 / 1024).toFixed(2)}GB`
}

