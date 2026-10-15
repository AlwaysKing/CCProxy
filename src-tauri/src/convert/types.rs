// 转换层用到的配置类型。
//
// 上游 cc-switch 的 types.rs 里还有一批应用层结构（Copilot 优化器、请求上下文、
// 各类 provider 元数据），转换链路只用得上 thinking 优化器这一小段，其余没有搬过来。
// 字段名与 serde 口径保持原样：将来要从上游同步这个文件时能直接对上。

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// 请求优化器配置：转换层只读 `thinking_optimizer` 这一个子开关，
/// 决定要不要在 Anthropic 侧动 thinking 相关字段。`enabled` 与 `cache_injection`
/// 是上游同名字段，留着是为了反序列化时字段不丢。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub thinking_optimizer: bool,
    #[serde(default = "default_true")]
    pub cache_injection: bool,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            thinking_optimizer: true,
            cache_injection: true,
        }
    }
}
