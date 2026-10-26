// 配置读写：~/.ccproxy/app.json。原子写（tmp + rename）、损坏自动备份 .bak 后回落默认配置，
// 不因配置问题崩溃。校验是纯函数，脱离界面即可单测。

use std::fs;
use std::path::{Path, PathBuf};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// 版本号只标记写出的配置形态，加载时不比对：新版读旧配置是兼容的（老结构走迁移，
// 模型条目两种写法都认），但旧版读新配置不兼容，所以不承诺降级回滚。
// v2：协议拆成三条，供应商从 protocols[].providers 提到顶层并声明 apiFormat。
// v4：供应商仍是全局池，地址与 token 挂在供应商上；模型列表只分 Claude / OpenAI 两类，
// 映射明确记录上游协议，路由直接按它决定透传或转换。
// v5：映射表顺序即匹配优先级；低版本配置升级时按旧优先级稳定排序一次，命中结果不变。
pub const CONFIG_VERSION: u32 = 6;
// 从这一版起列表顺序决定匹配优先级，低于它的配置需要按旧语义整理一次
const MAPPING_ORDER_VERSION: u32 = 5;
pub const DEFAULT_PORT: u16 = 18888;
// 客户端线格式三条线。协议只决定「客户端发来的模型名按哪张映射表查」；
// 模型属于 Claude 还是 OpenAI 由映射所属模型组决定；OpenAI 的 Chat / Responses 目标由映射协议决定。
pub const SUPPORTED_PROTOCOLS: [(&str, &str); 3] = [
    ("anthropic", "Anthropic"),
    ("openai_chat", "OpenAI Chat"),
    ("openai_responses", "OpenAI Responses"),
];
// 模型列表类型。供应商层面只区分 Claude 与 OpenAI；具体 Chat / Responses 由映射协议决定。
pub const API_FORMATS: [&str; 2] = ["anthropic", "openai"];
pub fn api_format_label(id: &str) -> &str {
    match id {
        "anthropic" => "Claude",
        "openai" => "OpenAI",
        _ => "未知格式",
    }
}
// 老配置里 OpenAI 是一条协议，现在拆成 chat / responses 两条。迁移时把老条目下的映射
// 复制到这两条上：旧行为下两条路径都能走通，复制是唯一无损的迁移方式
const LEGACY_PROTOCOL_ID: &str = "openai";
// 手动添加的客户端配置目录允许的类型；内置扫描位置不进配置
pub const CLIENT_DIR_KINDS: [&str; 2] = ["claude", "codex"];
// 输入模态白名单：以 codex models.json 的 input_modalities 为准，界面也只给这两项勾
const INPUT_MODALITIES: [&str; 2] = ["text", "image"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientDir {
    pub id: String,
    pub kind: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelBinding {
    pub id: String,
    pub path: String,
    pub client: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub port: u16,
    pub launch_at_login: bool,
    pub logs_enabled: bool,
    pub usage_enabled: bool,
    pub client_dirs: Vec<ClientDir>,
    pub client_token: String,
    pub project_model_bindings: Vec<ProjectModelBinding>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelGroup {
    pub id: String,
    // 模型列表类型：anthropic | openai
    pub api_format: String,
    pub models: Vec<Model>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub base_url: String,
    pub token: String,
    pub model_groups: Vec<ModelGroup>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub id: String,
    pub enabled: bool,
    pub client_model: String,
    pub provider_id: String,
    pub model_group_id: String,
    // anthropic | openai_chat | openai_responses
    pub upstream_protocol: String,
    pub real_model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Protocol {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub mappings: Vec<Mapping>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    pub settings: Settings,
    // 全局供应商池：三个协议共用。供应商是「一个上游端点 + 一种格式」，与客户端协议无关；
    // 挂在协议下面会让同一家上游在每个协议里各录一遍
    pub providers: Vec<Provider>,
    pub protocols: Vec<Protocol>,
}

impl AppConfig {
    pub fn protocol(&self, id: &str) -> Option<&Protocol> {
        self.protocols.iter().find(|item| item.id == id)
    }
}

pub fn create_default_config() -> AppConfig {
    AppConfig {
        version: CONFIG_VERSION,
        settings: Settings {
            port: DEFAULT_PORT,
            launch_at_login: false,
            logs_enabled: true,
            usage_enabled: true,
            client_dirs: Vec::new(),
            client_token: String::new(),
            project_model_bindings: Vec::new(),
        },
        protocols: SUPPORTED_PROTOCOLS
            .iter()
            .map(|(id, name)| Protocol {
                id: (*id).to_string(),
                name: (*name).to_string(),
                enabled: true,
                mappings: Vec::new(),
            })
            .collect(),
        providers: Vec::new(),
    }
}

pub fn create_config_id(prefix: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &id[..8])
}

// 客户端 token 是要填进客户端配置里的凭证，长得跟常见 API key 一致（sk- + 32 位十六进制），
// 与 createConfigId 生成的内部短 id 是两回事，别混用
pub fn create_client_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sk-{hex}")
}

fn is_plain_object(value: &Value) -> bool {
    value.is_object()
}

fn as_str(value: &Value) -> Option<&str> {
    value.as_str()
}

// 只认数字与纯数字串：true、[400000]、"1e5" 这些宽松强转出来的值一律拒掉
fn positive_integer(value: &Value) -> Option<u64> {
    let digits = |text: &str| -> Option<u64> {
        let text = text.trim();
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse::<u64>().ok().filter(|value| *value > 0)
    };
    match value {
        Value::Number(number) => {
            // 0 与负数都不算合法窗口；整数直接取，浮点退回按字面量判断
            if let Some(unsigned) = number.as_u64() {
                return if unsigned > 0 { Some(unsigned) } else { None };
            }
            digits(&number.to_string())
        }
        Value::String(text) => digits(text),
        _ => None,
    }
}

fn json_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

// 模型条目：名称必填；上下文窗口与模态/推理能力只有 OpenAI 协议的界面会填，
// 这里不按协议区分——填了且合法就留，非法只丢该字段并记 issue，不连整个模型一起丢。
// 兼容旧格式：字符串条目直接当模型名（老配置里存的就是这种）
fn normalize_model(raw: &Value, provider_id: &str, issues: &mut Vec<String>) -> Option<Model> {
    if let Some(text) = as_str(raw) {
        let name = text.trim();
        return if name.is_empty() {
            None
        } else {
            Some(Model {
                name: name.to_string(),
                context_window: None,
                max_context_window: None,
                input_modalities: None,
                reasoning: None,
            })
        };
    }
    if !is_plain_object(raw) {
        issues.push(format!("供应商 {provider_id} 下存在非法模型条目，已丢弃"));
        return None;
    }
    let name = raw
        .get("name")
        .and_then(as_str)
        .map(str::trim)
        .unwrap_or("");
    if name.is_empty() {
        issues.push(format!("供应商 {provider_id} 下有模型缺少 name，已丢弃"));
        return None;
    }

    let mut model = Model {
        name: name.to_string(),
        context_window: None,
        max_context_window: None,
        input_modalities: None,
        reasoning: None,
    };

    for field in ["contextWindow", "maxContextWindow"] {
        let raw_value = raw.get(field).unwrap_or(&Value::Null);
        // 留空表示「没配」，不当成 0 存
        if raw_value.is_null() || matches!(raw_value, Value::String(text) if text.is_empty()) {
            continue;
        }
        match positive_integer(raw_value) {
            Some(value) => {
                if field == "contextWindow" {
                    model.context_window = Some(value);
                } else {
                    model.max_context_window = Some(value);
                }
            }
            None => issues.push(format!(
                "模型 {name} 的 {field} 非法（{}），已忽略",
                json_text(raw_value)
            )),
        }
    }

    // null 与窗口字段一样当「没配」：手写的配置里 null 常常只是占位，不该弹警告
    if let Some(raw_modalities) = raw.get("inputModalities") {
        if !raw_modalities.is_null() {
            if let Some(items) = raw_modalities.as_array() {
                let mut modalities: Vec<String> = Vec::new();
                for item in items {
                    let name = item.as_str().unwrap_or("");
                    if INPUT_MODALITIES.contains(&name)
                        && !modalities.iter().any(|kept| kept == name)
                    {
                        modalities.push(name.to_string());
                    }
                }
                // 不强制 text 必须在：界面恒带上它，但手写的配置允许只声明 image
                if !modalities.is_empty() {
                    model.input_modalities = Some(modalities.clone());
                }
                let mut unique_raw: Vec<String> = Vec::new();
                for item in items {
                    let text = json_text(item);
                    if !unique_raw.contains(&text) {
                        unique_raw.push(text);
                    }
                }
                if modalities.len() < unique_raw.len() {
                    issues.push(format!(
                        "模型 {name} 的 inputModalities 含不支持的取值，已丢弃"
                    ));
                }
            } else {
                issues.push(format!("模型 {name} 的 inputModalities 不是数组，已忽略"));
            }
        }
    }

    // OpenAI 的模型只有勾/不勾两态，没有「未配置」——没勾选就是显式 false。
    // 这里非布尔不写回，是给手改配置兜底，不补默认值
    if let Some(reasoning) = raw.get("reasoning") {
        if let Some(flag) = reasoning.as_bool() {
            model.reasoning = Some(flag);
        } else if !reasoning.is_null() {
            issues.push(format!("模型 {name} 的 reasoning 不是布尔值，已忽略"));
        }
    }

    Some(model)
}

fn normalize_models(
    raw: Option<&Value>,
    provider_id: &str,
    issues: &mut Vec<String>,
) -> Vec<Model> {
    let mut models: Vec<Model> = Vec::new();
    if let Some(items) = raw.and_then(Value::as_array) {
        for item in items {
            let Some(model) = normalize_model(item, provider_id, issues) else {
                continue;
            };
            if models.iter().any(|kept| kept.name == model.name) {
                issues.push(format!(
                    "供应商 {provider_id} 存在重复模型 {}，已丢弃后者",
                    model.name
                ));
                continue;
            }
            models.push(model);
        }
    }
    models
}

fn canonical_model_format(value: &str) -> Option<&'static str> {
    match value {
        "anthropic" => Some("anthropic"),
        "openai" | "openai_chat" | "openai_responses" => Some("openai"),
        _ => None,
    }
}

fn protocol_model_format(protocol_id: &str) -> &'static str {
    if protocol_id == "anthropic" {
        "anthropic"
    } else {
        "openai"
    }
}

fn is_upstream_protocol(value: &str) -> bool {
    matches!(value, "anthropic" | "openai_chat" | "openai_responses")
}

fn protocol_allowed_for_group(group_format: &str, upstream_protocol: &str) -> bool {
    match group_format {
        "anthropic" => upstream_protocol == "anthropic",
        "openai" => matches!(upstream_protocol, "openai_chat" | "openai_responses"),
        _ => false,
    }
}

fn default_upstream_protocol(group_format: &str, protocol_id: &str) -> String {
    if group_format == "anthropic" {
        "anthropic".to_string()
    } else if protocol_id == "openai_responses" {
        "openai_responses".to_string()
    } else {
        "openai_chat".to_string()
    }
}

fn merge_model(existing: &mut Model, incoming: &Model) {
    if existing.context_window.is_none() {
        existing.context_window = incoming.context_window;
    }
    if existing.max_context_window.is_none() {
        existing.max_context_window = incoming.max_context_window;
    }
    if existing.input_modalities.is_none() {
        existing.input_modalities = incoming.input_modalities.clone();
    }
    if existing.reasoning.is_none() {
        existing.reasoning = incoming.reasoning;
    }
}

fn merge_model_group(existing: &mut ModelGroup, incoming: ModelGroup) {
    for model in incoming.models {
        if let Some(kept) = existing
            .models
            .iter_mut()
            .find(|item| item.name == model.name)
        {
            merge_model(kept, &model);
        } else {
            existing.models.push(model);
        }
    }
}

fn normalize_model_group(
    raw: &Value,
    fallback_format: Option<&str>,
    provider_id: &str,
    issues: &mut Vec<String>,
) -> Option<ModelGroup> {
    if !is_plain_object(raw) {
        issues.push(format!("供应商 {provider_id} 下存在非法模型组，已丢弃"));
        return None;
    }
    let declared = raw
        .get("apiFormat")
        .and_then(as_str)
        .map(str::trim)
        .unwrap_or("");
    let api_format = if let Some(format) = canonical_model_format(declared) {
        format.to_string()
    } else if let Some(format) = fallback_format.and_then(canonical_model_format) {
        format.to_string()
    } else {
        issues.push(format!(
            "供应商 {provider_id} 的模型组 apiFormat 非法（{}），已丢弃",
            json_text(&raw.get("apiFormat").cloned().unwrap_or(Value::Null))
        ));
        return None;
    };
    let id = format!("g_{api_format}");
    Some(ModelGroup {
        id,
        api_format,
        models: normalize_models(raw.get("models"), provider_id, issues),
    })
}

fn legacy_provider_formats(
    declared: &str,
    referenced_protocols: &[String],
    provider_id: &str,
    issues: &mut Vec<String>,
) -> Vec<String> {
    if let Some(format) = canonical_model_format(declared) {
        return vec![format.to_string()];
    }

    let mut formats: Vec<String> = Vec::new();
    for protocol_id in referenced_protocols {
        let Some(format) = canonical_model_format(protocol_id) else {
            continue;
        };
        if !formats.iter().any(|item| item == format) {
            formats.push(format.to_string());
        }
    }
    if !declared.is_empty() && declared != "auto" {
        issues.push(format!(
            "供应商 {provider_id} 的 apiFormat 非法（{declared}），已按现有映射推断模型组"
        ));
    }
    if formats.is_empty() {
        formats.push("openai".to_string());
    }
    formats
}

// 供应商始终是全局池。新结构直接读 modelGroups；旧结构把原来的单组配置展开成按协议划分的模型组。
fn normalize_provider(
    raw: &Value,
    referenced_protocols: &[String],
    fallback_format: Option<&str>,
    issues: &mut Vec<String>,
) -> Option<Provider> {
    if !is_plain_object(raw) {
        issues.push("存在非法供应商条目，已丢弃".to_string());
        return None;
    }
    let id = raw.get("id").and_then(as_str).unwrap_or("");
    if id.is_empty() {
        issues.push("有供应商缺少 id，已丢弃".to_string());
        return None;
    }
    let name = raw
        .get("name")
        .and_then(as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(id);
    let enabled = raw.get("enabled").and_then(Value::as_bool) != Some(false);
    let declared_base_url = raw
        .get("baseUrl")
        .and_then(as_str)
        .map(str::trim)
        .unwrap_or("");
    let group_base_urls: Vec<String> = raw
        .get("modelGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.get("baseUrl").and_then(as_str).map(str::trim))
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let base_url = if !declared_base_url.is_empty() {
        declared_base_url.to_string()
    } else if let Some(first) = group_base_urls.first() {
        if group_base_urls.iter().any(|value| value != first) {
            issues.push(format!(
                "供应商 {id} 的各模型组 baseUrl 不一致；无法合并回供应商级，已保留第一组地址并备份原配置"
            ));
        }
        first.clone()
    } else {
        String::new()
    };
    if base_url.is_empty() {
        issues.push(format!("供应商 {id} 缺少 baseUrl，已丢弃"));
        return None;
    }
    let declared_token = raw.get("token").and_then(as_str).unwrap_or("").to_string();
    let group_tokens: Vec<String> = raw
        .get("modelGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.get("token").and_then(as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let token = if !declared_token.is_empty() {
        declared_token
    } else if let Some(first) = group_tokens.first() {
        if group_tokens.iter().any(|value| value != first) {
            issues.push(format!(
                "供应商 {id} 的各模型组 token 不一致；无法合并回供应商级，已保留第一组 token 并备份原配置"
            ));
        }
        first.clone()
    } else {
        String::new()
    };

    let mut model_groups: Vec<ModelGroup> = Vec::new();
    if let Some(items) = raw.get("modelGroups").and_then(Value::as_array) {
        for item in items {
            let Some(group) = normalize_model_group(item, fallback_format, id, issues) else {
                continue;
            };
            if let Some(existing) = model_groups
                .iter_mut()
                .find(|kept| kept.api_format == group.api_format)
            {
                merge_model_group(existing, group);
                continue;
            }
            model_groups.push(group);
        }
    } else {
        let declared = raw
            .get("apiFormat")
            .and_then(as_str)
            .map(str::trim)
            .unwrap_or("");
        let formats = if let Some(format) = canonical_model_format(declared) {
            vec![format.to_string()]
        } else if let Some(format) = fallback_format.and_then(canonical_model_format) {
            vec![format.to_string()]
        } else {
            legacy_provider_formats(declared, referenced_protocols, id, issues)
        };
        let models = normalize_models(raw.get("models"), id, issues);
        for api_format in formats {
            if !API_FORMATS.contains(&api_format.as_str()) {
                continue;
            }
            model_groups.push(ModelGroup {
                id: format!("g_{api_format}"),
                api_format,
                models: models.clone(),
            });
        }
    }

    Some(Provider {
        id: id.to_string(),
        name: name.to_string(),
        enabled,
        base_url,
        token,
        model_groups,
    })
}

fn normalize_mapping(
    raw: &Value,
    protocol_id: &str,
    providers: &[Provider],
    issues: &mut Vec<String>,
) -> Option<Mapping> {
    if !is_plain_object(raw) {
        issues.push(format!("协议 {protocol_id} 下存在非法映射条目，已丢弃"));
        return None;
    }
    let mut missing: Vec<&str> = Vec::new();
    for field in ["id", "clientModel", "providerId", "realModel"] {
        let ok = raw
            .get(field)
            .and_then(as_str)
            .map(|text| !text.trim().is_empty())
            .unwrap_or(false);
        if !ok {
            missing.push(field);
        }
    }
    if !missing.is_empty() {
        issues.push(format!(
            "协议 {protocol_id} 下有映射缺少 {}，已丢弃",
            missing.join("/")
        ));
        return None;
    }
    let provider_id = raw.get("providerId").and_then(as_str).unwrap_or("");
    let Some(provider) = providers.iter().find(|item| item.id == provider_id) else {
        issues.push(format!(
            "协议 {protocol_id} 的映射 {} 指向不存在的供应商 {provider_id}，已丢弃",
            raw.get("clientModel").and_then(as_str).unwrap_or("")
        ));
        return None;
    };
    let requested_group = raw.get("modelGroupId").and_then(as_str).unwrap_or("");
    let requested_upstream = raw
        .get("upstreamProtocol")
        .and_then(as_str)
        .filter(|value| is_upstream_protocol(value));
    let group = if let Some(upstream_protocol) = requested_upstream {
        provider
            .model_groups
            .iter()
            .find(|item| item.api_format == protocol_model_format(upstream_protocol))
    } else if !requested_group.is_empty() {
        provider
            .model_groups
            .iter()
            .find(|item| item.id == requested_group)
            .or_else(|| {
                (requested_group.contains("openai")
                    || requested_group.contains("chat")
                    || requested_group.contains("responses"))
                .then(|| {
                    provider
                        .model_groups
                        .iter()
                        .find(|item| item.api_format == "openai")
                })
                .flatten()
            })
    } else {
        provider
            .model_groups
            .iter()
            .find(|item| item.api_format == protocol_model_format(protocol_id))
            .or_else(|| provider.model_groups.first())
    };
    let Some(group) = group else {
        issues.push(format!(
            "协议 {protocol_id} 的映射 {} 指向供应商 {provider_id}，但找不到可用的模型组，已丢弃",
            raw.get("clientModel").and_then(as_str).unwrap_or("")
        ));
        return None;
    };
    let upstream_protocol = if let Some(upstream_protocol) = requested_upstream {
        if !protocol_allowed_for_group(&group.api_format, upstream_protocol) {
            issues.push(format!(
                "协议 {protocol_id} 的映射 {} 选择了与模型组不匹配的上游协议 {upstream_protocol}，已丢弃",
                raw.get("clientModel").and_then(as_str).unwrap_or("")
            ));
            return None;
        }
        upstream_protocol.to_string()
    } else {
        default_upstream_protocol(&group.api_format, protocol_id)
    };
    Some(Mapping {
        id: raw.get("id").and_then(as_str).unwrap_or("").to_string(),
        enabled: raw.get("enabled").and_then(Value::as_bool) != Some(false),
        client_model: raw
            .get("clientModel")
            .and_then(as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        provider_id: provider_id.to_string(),
        model_group_id: group.id.clone(),
        upstream_protocol,
        real_model: raw
            .get("realModel")
            .and_then(as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    })
}

// 协议只留映射表：供应商已提到顶层，这里只校验映射指向的供应商是否存在（provider_ids 来自全局池）
fn build_protocol(
    raw: Option<&Value>,
    preset_id: &str,
    preset_name: &str,
    providers: &[Provider],
    issues: &mut Vec<String>,
) -> Protocol {
    let mut protocol = Protocol {
        id: preset_id.to_string(),
        name: preset_name.to_string(),
        enabled: true,
        mappings: Vec::new(),
    };
    let Some(raw) = raw.filter(|value| is_plain_object(value)) else {
        issues.push(format!("协议 {preset_id} 在配置中缺失，已用默认值补全"));
        return protocol;
    };
    protocol.enabled = raw.get("enabled").and_then(Value::as_bool) != Some(false);
    // 名字不读配置：协议是固定的三条预设，名字是预设的一部分（界面上也没有改名的地方）。
    // 读配置会出事——老配置里那条 openai 的 name 是 "OpenAI"，拆成 Chat 与 Responses 两条后
    // 两边都继承它，Tab 上就变成两个一模一样的「OpenAI」，分不出在配哪条

    let mut mapping_ids: Vec<String> = Vec::new();
    if let Some(items) = raw.get("mappings").and_then(Value::as_array) {
        for item in items {
            let Some(mapping) = normalize_mapping(item, preset_id, providers, issues) else {
                continue;
            };
            if mapping_ids.iter().any(|kept| *kept == mapping.id) {
                issues.push(format!(
                    "协议 {preset_id} 存在重复的映射 id {}，已丢弃后者",
                    mapping.id
                ));
                continue;
            }
            mapping_ids.push(mapping.id.clone());
            protocol.mappings.push(mapping);
        }
    }

    protocol
}

// v5 之前的匹配规则是：精确 > 通配（前缀越长越优先）> 兜底 *，与列表顺序无关。
// 升级时按这套规则稳定排序一次，保证升级前后实际命中的映射不变；之后顺序即可拖拽调整。
fn sort_mappings_legacy_priority(mappings: &mut [Mapping]) {
    fn legacy_key(pattern: &str) -> (u8, std::cmp::Reverse<usize>) {
        if pattern == "*" {
            (2, std::cmp::Reverse(0))
        } else if pattern.len() > 1 && pattern.ends_with('*') {
            (1, std::cmp::Reverse(pattern.len()))
        } else {
            (0, std::cmp::Reverse(0))
        }
    }
    // sort_by_key 是稳定排序：同优先级条目保持原有相对顺序
    mappings.sort_by_key(|mapping| legacy_key(&mapping.client_model));
}

// 手动目录条目：id/kind/path 齐全才留；path 必须是绝对路径——扫描按原样去找，
// 相对路径会随进程工作目录漂移，存下来就是隐患
fn normalize_client_dirs(raw: Option<&Value>, issues: &mut Vec<String>) -> Vec<ClientDir> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    if raw.is_null() {
        return Vec::new();
    }
    let Some(items) = raw.as_array() else {
        issues.push("settings.clientDirs 不是数组，已重置为空".to_string());
        return Vec::new();
    };

    let mut dirs: Vec<ClientDir> = Vec::new();
    for item in items {
        if !is_plain_object(item) {
            issues.push("settings.clientDirs 下存在非法目录条目，已丢弃".to_string());
            continue;
        }
        let id = item.get("id").and_then(as_str).unwrap_or("");
        if id.is_empty() {
            issues.push("settings.clientDirs 下有目录条目缺少 id，已丢弃".to_string());
            continue;
        }
        let dir_path = item
            .get("path")
            .and_then(as_str)
            .map(str::trim)
            .unwrap_or("");
        if dir_path.is_empty() || !Path::new(dir_path).is_absolute() {
            issues.push(format!("目录条目 {id} 的 path 要是绝对路径，已丢弃"));
            continue;
        }
        let kind = item.get("kind").and_then(as_str).unwrap_or("");
        if !CLIENT_DIR_KINDS.contains(&kind) {
            issues.push(format!(
                "目录条目 {id} 的 kind 非法（{}），已丢弃",
                json_text(&item.get("kind").cloned().unwrap_or(Value::Null))
            ));
            continue;
        }
        if dirs.iter().any(|kept| kept.path == dir_path) {
            issues.push(format!(
                "settings.clientDirs 存在重复目录 {dir_path}，已丢弃后者"
            ));
            continue;
        }
        dirs.push(ClientDir {
            id: id.to_string(),
            kind: kind.to_string(),
            path: dir_path.to_string(),
        });
    }
    dirs
}

fn normalize_project_path(raw: &str) -> Option<String> {
    let path = Path::new(raw.trim());
    if !path.is_absolute() {
        return None;
    }
    let normalized: PathBuf = path.components().collect();
    Some(normalized.to_string_lossy().to_string())
}

fn normalize_project_model(
    raw: Option<&Value>,
    binding_id: &str,
    issues: &mut Vec<String>,
) -> Option<String> {
    let raw = raw?;
    let Some(value) = raw.as_str() else {
        issues.push(format!("项目绑定 {binding_id} 的 model 不是字符串，已丢弃"));
        return None;
    };
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.contains('*') {
        issues.push(format!(
            "项目绑定 {binding_id} 的 model 不能是通配符，已丢弃"
        ));
        return None;
    }
    Some(value.to_string())
}

// 项目路径只做绝对路径与重复分隔符的纯字符串归一化，不访问文件系统：
// 项目临时离线不能让一次无关保存把绑定删掉。存在性在发现、添加和同步阶段检查。
fn normalize_project_bindings(
    settings: &Value,
    issues: &mut Vec<String>,
) -> Vec<ProjectModelBinding> {
    let current = settings.get("projectModelBindings");
    let legacy = settings.get("projectModelProjects");
    let Some(raw) = current.or(legacy) else {
        return Vec::new();
    };
    if raw.is_null() {
        return Vec::new();
    }
    let Some(items) = raw.as_array() else {
        issues.push("settings.projectModelBindings 不是数组，已重置为空".to_string());
        return Vec::new();
    };

    let mut bindings: Vec<ProjectModelBinding> = Vec::new();
    for item in items {
        if !is_plain_object(item) {
            issues.push("settings.projectModelBindings 下存在非法条目，已丢弃".to_string());
            continue;
        }
        let raw_path = item.get("path").and_then(as_str).unwrap_or("");
        let Some(path) = normalize_project_path(raw_path) else {
            issues.push(format!(
                "项目目录 path 必须是绝对路径（{}），已丢弃",
                json_text(&item.get("path").cloned().unwrap_or(Value::Null))
            ));
            continue;
        };

        let is_legacy = current.is_none();
        let candidates: Vec<(&str, Option<&Value>)> = if is_legacy {
            vec![
                ("codex", item.get("codexModel")),
                ("claude", item.get("claudeModel")),
            ]
        } else {
            let client = item.get("client").and_then(as_str).unwrap_or("");
            vec![(client, item.get("model"))]
        };
        for (client, raw_model) in candidates {
            if client != "codex" && client != "claude" {
                issues.push(format!(
                    "项目绑定 {path} 的 client 非法（{client}），已丢弃"
                ));
                continue;
            }
            if bindings
                .iter()
                .any(|kept| kept.path == path && kept.client == client)
            {
                issues.push(format!("存在重复的项目绑定 {path} ({client})，已丢弃后者"));
                continue;
            }
            let raw_id = item.get("id").and_then(as_str).unwrap_or("").trim();
            let mut id = if raw_id.is_empty() {
                create_config_id("b")
            } else if is_legacy {
                create_config_id("b")
            } else {
                raw_id.to_string()
            };
            if bindings.iter().any(|kept| kept.id == id) {
                issues.push(format!("存在重复的项目绑定 id {id}，已重新生成"));
                loop {
                    let candidate = create_config_id("b");
                    if !bindings.iter().any(|kept| kept.id == candidate) {
                        id = candidate;
                        break;
                    }
                }
            }
            let Some(model) = normalize_project_model(raw_model, &id, issues) else {
                continue;
            };
            bindings.push(ProjectModelBinding {
                id,
                path: path.clone(),
                client: client.to_string(),
                model,
            });
        }
    }
    if current.is_none() && !bindings.is_empty() {
        issues.push("项目模型绑定已从旧结构升级为按客户端拆分".to_string());
    }
    bindings
}

// JS 的 Number() 语义：数字原样、字符串按数值解析（空串是 0）、布尔转 0/1、null 是 0
fn to_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Some(0.0)
            } else {
                trimmed.parse::<f64>().ok()
            }
        }
        Value::Bool(true) => Some(1.0),
        Value::Bool(false) | Value::Null => Some(0.0),
        _ => None,
    }
}

// 两份供应商定义是否等价：等价就直接复用，不等价就拆成两个。
// 模型组的连接参数或格式不同就不能合并，否则映射会悄悄改走另一条上游。
fn same_provider(left: &Provider, right: &Provider) -> bool {
    left.name == right.name
        && left.enabled == right.enabled
        && left.base_url == right.base_url
        && left.token == right.token
        && left.model_groups.len() == right.model_groups.len()
        && left.model_groups.iter().all(|left_group| {
            right
                .model_groups
                .iter()
                .any(|right_group| left_group.api_format == right_group.api_format)
        })
}

// 给重名的供应商取一个没被占用的 id：p_ab12 → p_ab12_2
fn unique_provider_id(base: &str, taken: &[String]) -> String {
    let mut index = 2usize;
    loop {
        let candidate = format!("{base}_{index}");
        if !taken.iter().any(|kept| *kept == candidate) {
            return candidate;
        }
        index += 1;
    }
}

// 把某个协议条目里的映射改指到改名后的供应商（老结构迁移专用）
fn rewrite_mapping_providers(protocol: &mut Value, renamed: &[(String, String)]) {
    let Some(mappings) = protocol.get_mut("mappings").and_then(Value::as_array_mut) else {
        return;
    };
    for mapping in mappings.iter_mut() {
        let Some(object) = mapping.as_object_mut() else {
            continue;
        };
        let Some(current) = object.get("providerId").and_then(Value::as_str) else {
            continue;
        };
        if let Some((_, next)) = renamed.iter().find(|(old, _)| old == current) {
            object.insert("providerId".to_string(), Value::String(next.clone()));
        }
    }
}

fn expanded_protocol_ids(protocol_id: &str) -> Vec<String> {
    match protocol_id {
        LEGACY_PROTOCOL_ID => vec!["openai_chat".to_string(), "openai_responses".to_string()],
        other => vec![other.to_string()],
    }
}

fn referenced_protocols(raw_protocols: &[Value], provider_id: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for protocol in raw_protocols {
        let Some(protocol_id) = protocol.get("id").and_then(as_str) else {
            continue;
        };
        let Some(mappings) = protocol.get("mappings").and_then(Value::as_array) else {
            continue;
        };
        if !mappings
            .iter()
            .any(|mapping| mapping.get("providerId").and_then(as_str) == Some(provider_id))
        {
            continue;
        }
        for expanded in expanded_protocol_ids(protocol_id) {
            if !result.iter().any(|item| item == &expanded) {
                result.push(expanded);
            }
        }
    }
    result
}

// 配置是不是老结构：版本未到当前版本，协议下挂着 providers，还留着拆分前的 openai 协议，
// 或顶层供应商仍是 v2 的单组结构。
fn has_legacy_shape(raw: &Value) -> bool {
    if raw.get("version").and_then(Value::as_u64).unwrap_or(0) < CONFIG_VERSION as u64 {
        return true;
    }
    let Some(protocols) = raw.get("protocols").and_then(Value::as_array) else {
        return false;
    };
    if protocols.iter().any(|item| {
        is_plain_object(item)
            && (item.get("id").and_then(as_str) == Some(LEGACY_PROTOCOL_ID)
                || item.get("providers").is_some())
    }) {
        return true;
    }
    raw.get("providers")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                if !is_plain_object(item) {
                    return false;
                }
                if item.get("modelGroups").is_none() || item.get("baseUrl").is_none() {
                    return true;
                }
                item.get("modelGroups")
                    .and_then(Value::as_array)
                    .is_some_and(|groups| {
                        groups.iter().any(|group| {
                            group
                                .get("apiFormat")
                                .and_then(as_str)
                                .is_some_and(|value| !API_FORMATS.contains(&value))
                        })
                    })
            })
        })
}

pub struct Validated {
    pub ok: bool,
    pub config: Option<AppConfig>,
    pub issues: Vec<String>,
}

// 校验并归一化：能修的直接修，修不了的条目丢掉并记 issue。
// 只有「根节点不是对象」才判定为损坏（由调用方备份并回落默认配置）。
pub fn validate_config(raw: &Value) -> Validated {
    if !is_plain_object(raw) {
        return Validated {
            ok: false,
            config: None,
            issues: vec!["配置根节点不是对象".to_string()],
        };
    }
    let mut issues: Vec<String> = Vec::new();
    let empty = json!({});
    let settings = raw
        .get("settings")
        .filter(|value| is_plain_object(value))
        .unwrap_or(&empty);

    let port_value = to_number(settings.get("port").unwrap_or(&Value::Null));
    let port = port_value.filter(|value| {
        value.is_finite() && value.fract() == 0.0 && *value >= 1.0 && *value <= 65535.0
    });
    if port.is_none() {
        issues.push(format!(
            "settings.port 非法（{}），已回落 {DEFAULT_PORT}",
            json_text(settings.get("port").unwrap_or(&Value::Null))
        ));
    }

    let raw_version = raw.get("version").and_then(Value::as_u64).unwrap_or(0);
    let raw_protocols = raw.get("protocols").and_then(Value::as_array).cloned();
    if raw_protocols.is_none() {
        issues.push("protocols 不是数组，已重置为默认协议".to_string());
    }
    // 迁移时要就地改老结构里的映射指向（见下面的重名处理），所以这里拿可变的副本
    let mut raw_protocols = raw_protocols.unwrap_or_default();

    // 供应商池：先读顶层 providers，再把老形态（挂在协议下面）提升上来。
    // v2 的单组供应商会展开成 modelGroups；auto 根据现有映射推断，旧 openai 默认展开成
    // Chat 与 Responses 两组，避免把原先可直通的两条路径静默改成转换。
    let mut providers: Vec<Provider> = Vec::new();
    let mut provider_ids: Vec<String> = Vec::new();
    if let Some(items) = raw.get("providers").and_then(Value::as_array) {
        for item in items {
            let id = item.get("id").and_then(as_str).unwrap_or("");
            let referenced = referenced_protocols(&raw_protocols, id);
            let Some(provider) = normalize_provider(item, &referenced, None, &mut issues) else {
                continue;
            };
            if provider_ids.iter().any(|kept| *kept == provider.id) {
                issues.push(format!("存在重复的供应商 id {}，已丢弃后者", provider.id));
                continue;
            }
            provider_ids.push(provider.id.clone());
            providers.push(provider);
        }
    }
    let mut hoisted_count = 0usize;
    // 只有顶层池为空时才从协议下提升：两种形态同时存在说明文件被手工改过（或者只写了一半），
    // 这时以顶层为准，不能把协议下那份老数据再提升一遍——那会把已经删掉的供应商救回来
    let nested_present = raw_protocols
        .iter()
        .any(|item| is_plain_object(item) && item.get("providers").is_some());
    let hoist_legacy = providers.is_empty();
    if !hoist_legacy && nested_present {
        issues.push(
            "配置里同时有顶层 providers 与协议下的 providers：以顶层为准，协议下的那份已忽略"
                .to_string(),
        );
    }
    for item in raw_protocols.iter_mut() {
        if !hoist_legacy {
            continue;
        }
        if !is_plain_object(item) {
            continue;
        }
        let Some(entries) = item.get("providers").and_then(Value::as_array).cloned() else {
            continue;
        };
        let owner = item.get("id").and_then(as_str).unwrap_or("");
        let fallback = match owner {
            "anthropic" => Some("anthropic"),
            "openai_chat" => Some("openai_chat"),
            "openai_responses" => Some("openai_responses"),
            _ => None,
        };
        let referenced = expanded_protocol_ids(owner);
        // 老结构里供应商 id 只在协议内唯一，两个协议下可以各有一份同名但指向不同上游的条目
        // （手改配置会这样）。提升到全局池时这种重名要改名保留：直接丢掉会让映射悄悄指到
        // 另一家上游上去。改名之后，该协议内的映射一并改指过去
        let mut renamed: Vec<(String, String)> = Vec::new();
        for entry in &entries {
            let Some(mut provider) = normalize_provider(entry, &referenced, fallback, &mut issues)
            else {
                continue;
            };
            if let Some(existing) = providers.iter().find(|kept| kept.id == provider.id) {
                if same_provider(existing, &provider) {
                    continue;
                }
                let next_id = unique_provider_id(&provider.id, &provider_ids);
                issues.push(format!(
                    "供应商 {} 在老配置的多个协议下定义不一致（上游地址、token 或上游格式不同），已拆成 {}，各协议内的映射按原指向改指",
                    provider.id, next_id
                ));
                renamed.push((provider.id.clone(), next_id.clone()));
                provider.id = next_id;
            }
            provider_ids.push(provider.id.clone());
            providers.push(provider);
            hoisted_count += 1;
        }
        if !renamed.is_empty() {
            rewrite_mapping_providers(item, &renamed);
        }
    }
    if hoisted_count > 0 {
        issues.push(format!(
            "供应商已升为全局（三个协议共用），从老配置提升了 {hoisted_count} 个；模型已按上游格式分组"
        ));
    }

    let mut migrated_legacy = false;
    let mut protocols: Vec<Protocol> = SUPPORTED_PROTOCOLS
        .iter()
        .map(|(id, name)| {
            let mut matched = raw_protocols
                .iter()
                .find(|item| is_plain_object(item) && item.get("id").and_then(as_str) == Some(*id));
            // 老配置里 OpenAI 只有一条：拆出来的 chat 与 responses 两条都从它复制
            if matched.is_none() && (*id == "openai_chat" || *id == "openai_responses") {
                matched = raw_protocols.iter().find(|item| {
                    is_plain_object(item)
                        && item.get("id").and_then(as_str) == Some(LEGACY_PROTOCOL_ID)
                });
                if matched.is_some() {
                    migrated_legacy = true;
                }
            }
            build_protocol(matched, id, name, &providers, &mut issues)
        })
        .collect();
    // 旧版配置的列表顺序不参与路由，升级时整理成旧语义下的优先级顺序
    if raw_version < MAPPING_ORDER_VERSION as u64 {
        for protocol in &mut protocols {
            sort_mappings_legacy_priority(&mut protocol.mappings);
        }
    }
    if migrated_legacy {
        issues.push(
            "OpenAI 协议已拆成 OpenAI Chat 与 OpenAI Responses 两条，原映射表已各复制一份"
                .to_string(),
        );
    }

    // 老的 openai 条目与拆分后的条目同时存在时，以拆分后的为准。这不算配置错误，
    // 但要说一声，免得用户以为老条目里的映射还在生效
    let has_split_entry = raw_protocols.iter().any(|item| {
        is_plain_object(item)
            && matches!(
                item.get("id").and_then(as_str),
                Some("openai_chat") | Some("openai_responses")
            )
    });
    let has_legacy_entry = raw_protocols.iter().any(|item| {
        is_plain_object(item) && item.get("id").and_then(as_str) == Some(LEGACY_PROTOCOL_ID)
    });
    if has_split_entry && has_legacy_entry {
        issues.push("配置里同时有老的 openai 协议条目与拆分后的条目：以拆分后的为准，老条目及其映射已被忽略".to_string());
    }

    let config = AppConfig {
        version: CONFIG_VERSION,
        settings: Settings {
            port: port.map(|value| value as u16).unwrap_or(DEFAULT_PORT),
            launch_at_login: settings.get("launchAtLogin").and_then(Value::as_bool) == Some(true),
            // 开关缺省即开启：老配置里没有这两个键，读出来必须还是记录，行为不回退
            logs_enabled: settings.get("logsEnabled").and_then(Value::as_bool) != Some(false),
            usage_enabled: settings.get("usageEnabled").and_then(Value::as_bool) != Some(false),
            client_dirs: normalize_client_dirs(settings.get("clientDirs"), &mut issues),
            client_token: settings
                .get("clientToken")
                .and_then(as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            project_model_bindings: normalize_project_bindings(settings, &mut issues),
        },
        providers,
        protocols,
    };

    for item in &raw_protocols {
        let id = if is_plain_object(item) {
            item.get("id").cloned().unwrap_or(Value::Null)
        } else {
            item.clone()
        };
        let known = id
            .as_str()
            .map(|text| {
                // 老的 openai 条目不算「不支持」：它的映射已经复制到拆出来的两条协议上
                text == LEGACY_PROTOCOL_ID || SUPPORTED_PROTOCOLS.iter().any(|(id, _)| *id == text)
            })
            .unwrap_or(false);
        if !known {
            let label = if id.is_null() && is_plain_object(item) {
                "undefined".to_string()
            } else {
                json_text(&id)
            };
            issues.push(format!("丢弃不支持的协议：{label}"));
        }
    }

    Validated {
        ok: true,
        config: Some(config),
        issues,
    }
}

pub struct LoadResult {
    pub config: AppConfig,
    pub issues: Vec<String>,
    pub recovered: bool,
    pub backup_path: Option<String>,
    pub first_run: bool,
}

pub struct ConfigManager {
    base_dir: PathBuf,
    config_path: PathBuf,
    temp_path: PathBuf,
}

impl ConfigManager {
    pub fn new(dir: Option<PathBuf>) -> Self {
        let base_dir = dir.unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".ccproxy")
        });
        let config_path = base_dir.join("app.json");
        let temp_path = base_dir.join("app.json.tmp");
        Self {
            base_dir,
            config_path,
            temp_path,
        }
    }

    pub fn path(&self) -> String {
        self.config_path.to_string_lossy().to_string()
    }

    pub fn dir(&self) -> PathBuf {
        self.base_dir.clone()
    }

    // 损坏配置留一份 .bak 再回落默认值，便于用户手工找回
    fn backup_broken_config(&self) -> Option<String> {
        if !self.config_path.exists() {
            return None;
        }
        let backup_path = self.base_dir.join("app.json.bak");
        fs::rename(&self.config_path, &backup_path).ok()?;
        Some(backup_path.to_string_lossy().to_string())
    }

    // 老结构的配置会升级成新结构：供应商提升到顶层，模型按上游格式分组。
    // 升级前先把原文件复制一份——万一新版读出来不对，用户手上还有一份旧结构可以照着改回去。
    // 复制成功再把新结构落盘，免得每次启动都重跑一遍迁移。
    // 备份或写回失败都只记 issue，不打断这次启动：内存里已经是升级后的结构了
    fn upgrade_legacy_config(&self, raw: &Value, config: &AppConfig, issues: &mut Vec<String>) {
        if !has_legacy_shape(raw) {
            return;
        }
        let version = raw.get("version").and_then(Value::as_u64).unwrap_or(0);
        let backup_name = format!("app.json.v{}.bak", version.max(1));
        let mut backup = self.base_dir.join(&backup_name);
        let mut suffix = 1;
        while backup.exists() {
            backup = self.base_dir.join(format!("{backup_name}.{suffix}"));
            suffix += 1;
        }
        let backup_note = match fs::copy(&self.config_path, &backup) {
            Ok(_) => format!("原文件已备份为 {}", backup.to_string_lossy()),
            Err(error) => format!("复制原文件失败：{error}"),
        };
        let encoded = serde_json::to_value(config).unwrap_or(Value::Null);
        // v5 只新增项目模型设置，没有重排映射；v4 才是映射顺序迁移，更老的版本是结构升级
        let note = if version == 5 {
            "已把旧版配置升级到 v6（新增项目默认模型设置，映射表保持原顺序）"
        } else if version >= 4 {
            "已把旧版配置升级为当前版本（映射表已按原优先级整理，之后可用拖拽调整）"
        } else {
            "已把旧版配置升级为新结构（供应商保持全局池、模型列表按 Claude / OpenAI 归类）"
        };
        match self.save(&encoded) {
            Ok((_, save_issues)) => {
                issues.push(format!("{note}。{backup_note}"));
                issues.extend(save_issues);
            }
            Err(error) => issues.push(format!(
                "{note}，但写回失败（{error}），本次仍按升级后的结构运行。{backup_note}"
            )),
        }
    }

    pub fn load(&self) -> LoadResult {
        let text = match fs::read_to_string(&self.config_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return LoadResult {
                    config: create_default_config(),
                    issues: Vec::new(),
                    recovered: false,
                    backup_path: None,
                    first_run: true,
                }
            }
            Err(error) => {
                let backup_path = self.backup_broken_config();
                return LoadResult {
                    config: create_default_config(),
                    issues: vec![error.to_string()],
                    recovered: true,
                    backup_path,
                    first_run: false,
                };
            }
        };

        let raw: Value = match serde_json::from_str(&text) {
            Ok(raw) => raw,
            Err(error) => {
                let backup_path = self.backup_broken_config();
                return LoadResult {
                    config: create_default_config(),
                    issues: vec![format!("配置不是合法 JSON：{error}")],
                    recovered: true,
                    backup_path,
                    first_run: false,
                };
            }
        };

        let mut result = validate_config(&raw);
        if result.ok {
            if let Some(config) = result.config.as_ref() {
                self.upgrade_legacy_config(&raw, config, &mut result.issues);
            }
        }
        match result.config {
            Some(config) if result.ok => LoadResult {
                config,
                issues: result.issues,
                recovered: false,
                backup_path: None,
                first_run: false,
            },
            _ => {
                let backup_path = self.backup_broken_config();
                LoadResult {
                    config: create_default_config(),
                    issues: result.issues,
                    recovered: true,
                    backup_path,
                    first_run: false,
                }
            }
        }
    }

    // 原子写：先写 .tmp 再 rename，避免写一半掉电留下半个文件
    pub fn save(&self, next: &Value) -> Result<(AppConfig, Vec<String>), String> {
        let result = validate_config(next);
        let Some(config) = result.config else {
            return Err(format!("配置校验失败：{}", result.issues.join("；")));
        };

        fs::create_dir_all(&self.base_dir).map_err(|error| error.to_string())?;
        set_mode(&self.base_dir, 0o700);

        let mut text = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
        text.push('\n');
        fs::write(&self.temp_path, text).map_err(|error| error.to_string())?;
        set_mode(&self.temp_path, 0o600);
        fs::rename(&self.temp_path, &self.config_path).map_err(|error| error.to_string())?;

        Ok((config, result.issues))
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}
