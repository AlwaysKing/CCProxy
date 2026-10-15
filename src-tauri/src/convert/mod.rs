// 协议转换层：在 anthropic / openai_chat / openai_responses 三种线格式之间互转。
//
// 这些模块是从 cc-switch（github.com/farion1231/cc-switch，基线 51ac86f1）的本地代理里
// 搬过来的，只保留了三种格式那部分：依赖 Gemini 的模块（gemini_schema / gemini_shadow /
// streaming_gemini / transform_gemini）没有搬。
//
// 转换层只做协议结构转换，不管模型映射、认证、重试、计费——那些留在 proxy 层。
// 上游返回异常、错误体、截断流和「2xx 但语义失败」都算失败，不能转成静默成功的空回复。
//
// 整棵树关掉 dead_code 告警：vendor 过来的代码里有一批当前用不到的辅助函数（比如只在
// 上游「Responses 直通 + xAI 网关」那条路上用的 sanitize 模块），留着是为了将来同步上游时
// 直接比对、以及第二批功能（客户端是 Chat 的转换）能取用。告警该反映我们自己写的代码
#![allow(dead_code)]

pub mod provider;

pub(crate) mod error;
pub(crate) mod json_canonical;
pub(crate) mod providers;
pub(crate) mod sse;
pub(crate) mod stream;
pub(crate) mod thinking_optimizer;
pub(crate) mod tool_media;
pub(crate) mod types;
pub(crate) mod usage;
pub(crate) mod usage_shape;
