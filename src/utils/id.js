// 生成配置条目 id：前缀 + 8 位随机串，与主进程 config-manager 的 createConfigId 形态一致
export function newId(prefix) {
  return `${prefix}_${crypto.randomUUID().replace(/-/g, '').slice(0, 8)}`
}
