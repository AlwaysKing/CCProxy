// 转换层需要的一个小结构：Codex Responses → Chat Completions 的 reasoning 能力描述。
//
// 上游 cc-switch 把它放在 provider.rs（那个文件里还有 ProviderMeta 等应用层结构），
// 这里只留这一个结构 + 既有的 serde 字段口径，方便将来从上游同步。

use serde::{Deserialize, Serialize};

/// Codex Responses -> Chat Completions 的 reasoning 能力描述。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CodexChatReasoningConfig {
    #[serde(rename = "supportsThinking", skip_serializing_if = "Option::is_none")]
    pub supports_thinking: Option<bool>,
    #[serde(rename = "supportsEffort", skip_serializing_if = "Option::is_none")]
    pub supports_effort: Option<bool>,
    #[serde(rename = "thinkingParam", skip_serializing_if = "Option::is_none")]
    pub thinking_param: Option<String>,
    #[serde(rename = "effortParam", skip_serializing_if = "Option::is_none")]
    pub effort_param: Option<String>,
    #[serde(rename = "effortValueMode", skip_serializing_if = "Option::is_none")]
    pub effort_value_mode: Option<String>,
    #[serde(rename = "outputFormat", skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// 运行时字段（不持久化）：当前请求模型声明的合法 effort 档位。
    /// Some → 把 effort 钳到合法档；None → 不发 effort 字段。
    #[serde(skip)]
    pub effort_levels: Option<Vec<String>>,
}
