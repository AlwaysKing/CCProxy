// 配置读写：~/.ccproxy/app.json。原子写（tmp + rename）、损坏自动备份 .bak 后回落默认配置，
// 不因配置问题崩溃。校验是纯函数，脱离界面即可单测。

use std::fs;
use std::path::{Path, PathBuf};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// 版本号只标记写出的配置形态，加载时不比对：新版读旧配置是兼容的（模型条目两种写法都认），
// 但旧版读新配置不兼容（对象条目会被旧的字符串过滤静默滤掉），所以不承诺降级回滚
pub const CONFIG_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 18888;
// v1 只支持这两个协议：识别器与适配器都按这个集合工作，配置里出现别的协议没有意义
pub const SUPPORTED_PROTOCOLS: [(&str, &str); 2] = [("anthropic", "Anthropic"), ("openai", "OpenAI")];
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
pub struct Settings {
    pub port: u16,
    pub launch_at_login: bool,
    pub logs_enabled: bool,
    pub usage_enabled: bool,
    pub client_dirs: Vec<ClientDir>,
    pub client_token: String,
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
pub struct Provider {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub base_url: String,
    pub token: String,
    pub models: Vec<Model>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub id: String,
    pub enabled: bool,
    pub client_model: String,
    pub provider_id: String,
    pub real_model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Protocol {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub providers: Vec<Provider>,
    pub mappings: Vec<Mapping>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    pub settings: Settings,
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
        },
        protocols: SUPPORTED_PROTOCOLS
            .iter()
            .map(|(id, name)| Protocol {
                id: (*id).to_string(),
                name: (*name).to_string(),
                enabled: true,
                providers: Vec::new(),
                mappings: Vec::new(),
            })
            .collect(),
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
fn normalize_model(raw: &Value, protocol_id: &str, provider_id: &str, issues: &mut Vec<String>) -> Option<Model> {
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
        issues.push(format!("协议 {protocol_id} 的供应商 {provider_id} 下存在非法模型条目，已丢弃"));
        return None;
    }
    let name = raw.get("name").and_then(as_str).map(str::trim).unwrap_or("");
    if name.is_empty() {
        issues.push(format!("协议 {protocol_id} 的供应商 {provider_id} 下有模型缺少 name，已丢弃"));
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
            None => issues.push(format!("模型 {name} 的 {field} 非法（{}），已忽略", json_text(raw_value))),
        }
    }

    // null 与窗口字段一样当「没配」：手写的配置里 null 常常只是占位，不该弹警告
    if let Some(raw_modalities) = raw.get("inputModalities") {
        if !raw_modalities.is_null() {
            if let Some(items) = raw_modalities.as_array() {
                let mut modalities: Vec<String> = Vec::new();
                for item in items {
                    let name = item.as_str().unwrap_or("");
                    if INPUT_MODALITIES.contains(&name) && !modalities.iter().any(|kept| kept == name) {
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
                    issues.push(format!("模型 {name} 的 inputModalities 含不支持的取值，已丢弃"));
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

fn normalize_provider(raw: &Value, protocol_id: &str, issues: &mut Vec<String>) -> Option<Provider> {
    if !is_plain_object(raw) {
        issues.push(format!("协议 {protocol_id} 下存在非法供应商条目，已丢弃"));
        return None;
    }
    let id = raw.get("id").and_then(as_str).unwrap_or("");
    if id.is_empty() {
        issues.push(format!("协议 {protocol_id} 下有供应商缺少 id，已丢弃"));
        return None;
    }
    let base_url = raw.get("baseUrl").and_then(as_str).map(str::trim).unwrap_or("");
    if base_url.is_empty() {
        issues.push(format!("协议 {protocol_id} 的供应商 {id} 缺少 baseUrl，已丢弃"));
        return None;
    }

    let mut models: Vec<Model> = Vec::new();
    if let Some(items) = raw.get("models").and_then(Value::as_array) {
        for item in items {
            let Some(model) = normalize_model(item, protocol_id, id, issues) else {
                continue;
            };
            if models.iter().any(|kept| kept.name == model.name) {
                issues.push(format!("协议 {protocol_id} 的供应商 {id} 存在重复模型 {}，已丢弃后者", model.name));
                continue;
            }
            models.push(model);
        }
    }

    let name = raw.get("name").and_then(as_str).map(str::trim).filter(|text| !text.is_empty()).unwrap_or(id);
    Some(Provider {
        id: id.to_string(),
        name: name.to_string(),
        enabled: raw.get("enabled").and_then(Value::as_bool) != Some(false),
        base_url: base_url.to_string(),
        token: raw.get("token").and_then(as_str).unwrap_or("").to_string(),
        models,
    })
}

fn normalize_mapping(
    raw: &Value,
    protocol_id: &str,
    provider_ids: &[String],
    issues: &mut Vec<String>,
) -> Option<Mapping> {
    if !is_plain_object(raw) {
        issues.push(format!("协议 {protocol_id} 下存在非法映射条目，已丢弃"));
        return None;
    }
    let mut missing: Vec<&str> = Vec::new();
    for field in ["id", "clientModel", "providerId", "realModel"] {
        let ok = raw.get(field).and_then(as_str).map(|text| !text.trim().is_empty()).unwrap_or(false);
        if !ok {
            missing.push(field);
        }
    }
    if !missing.is_empty() {
        issues.push(format!("协议 {protocol_id} 下有映射缺少 {}，已丢弃", missing.join("/")));
        return None;
    }
    let provider_id = raw.get("providerId").and_then(as_str).unwrap_or("");
    if !provider_ids.iter().any(|kept| kept == provider_id) {
        issues.push(format!(
            "协议 {protocol_id} 的映射 {} 指向不存在的供应商 {provider_id}，已丢弃",
            raw.get("clientModel").and_then(as_str).unwrap_or("")
        ));
        return None;
    }
    Some(Mapping {
        id: raw.get("id").and_then(as_str).unwrap_or("").to_string(),
        enabled: raw.get("enabled").and_then(Value::as_bool) != Some(false),
        client_model: raw.get("clientModel").and_then(as_str).unwrap_or("").trim().to_string(),
        provider_id: provider_id.to_string(),
        real_model: raw.get("realModel").and_then(as_str).unwrap_or("").trim().to_string(),
    })
}

fn build_protocol(raw: Option<&Value>, preset_id: &str, preset_name: &str, issues: &mut Vec<String>) -> Protocol {
    let mut protocol = Protocol {
        id: preset_id.to_string(),
        name: preset_name.to_string(),
        enabled: true,
        providers: Vec::new(),
        mappings: Vec::new(),
    };
    let Some(raw) = raw.filter(|value| is_plain_object(value)) else {
        issues.push(format!("协议 {preset_id} 在配置中缺失，已用默认值补全"));
        return protocol;
    };
    protocol.enabled = raw.get("enabled").and_then(Value::as_bool) != Some(false);
    if let Some(name) = raw.get("name").and_then(as_str).map(str::trim).filter(|text| !text.is_empty()) {
        protocol.name = name.to_string();
    }

    let mut provider_ids: Vec<String> = Vec::new();
    if let Some(items) = raw.get("providers").and_then(Value::as_array) {
        for item in items {
            let Some(provider) = normalize_provider(item, preset_id, issues) else {
                continue;
            };
            if provider_ids.iter().any(|kept| *kept == provider.id) {
                issues.push(format!("协议 {preset_id} 存在重复的供应商 id {}，已丢弃后者", provider.id));
                continue;
            }
            provider_ids.push(provider.id.clone());
            protocol.providers.push(provider);
        }
    }

    let mut mapping_ids: Vec<String> = Vec::new();
    if let Some(items) = raw.get("mappings").and_then(Value::as_array) {
        for item in items {
            let Some(mapping) = normalize_mapping(item, preset_id, &provider_ids, issues) else {
                continue;
            };
            if mapping_ids.iter().any(|kept| *kept == mapping.id) {
                issues.push(format!("协议 {preset_id} 存在重复的映射 id {}，已丢弃后者", mapping.id));
                continue;
            }
            mapping_ids.push(mapping.id.clone());
            protocol.mappings.push(mapping);
        }
    }

    protocol
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
        let dir_path = item.get("path").and_then(as_str).map(str::trim).unwrap_or("");
        if dir_path.is_empty() || !Path::new(dir_path).is_absolute() {
            issues.push(format!("目录条目 {id} 的 path 要是绝对路径，已丢弃"));
            continue;
        }
        let kind = item.get("kind").and_then(as_str).unwrap_or("");
        if !CLIENT_DIR_KINDS.contains(&kind) {
            issues.push(format!("目录条目 {id} 的 kind 非法（{}），已丢弃", json_text(&item.get("kind").cloned().unwrap_or(Value::Null))));
            continue;
        }
        if dirs.iter().any(|kept| kept.path == dir_path) {
            issues.push(format!("settings.clientDirs 存在重复目录 {dir_path}，已丢弃后者"));
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
    let settings = raw.get("settings").filter(|value| is_plain_object(value)).unwrap_or(&empty);

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

    let raw_protocols = raw.get("protocols").and_then(Value::as_array).cloned();
    if raw_protocols.is_none() {
        issues.push("protocols 不是数组，已重置为默认协议".to_string());
    }
    let raw_protocols = raw_protocols.unwrap_or_default();

    let config = AppConfig {
        version: CONFIG_VERSION,
        settings: Settings {
            port: port.map(|value| value as u16).unwrap_or(DEFAULT_PORT),
            launch_at_login: settings.get("launchAtLogin").and_then(Value::as_bool) == Some(true),
            // 开关缺省即开启：老配置里没有这两个键，读出来必须还是记录，行为不回退
            logs_enabled: settings.get("logsEnabled").and_then(Value::as_bool) != Some(false),
            usage_enabled: settings.get("usageEnabled").and_then(Value::as_bool) != Some(false),
            client_dirs: normalize_client_dirs(settings.get("clientDirs"), &mut issues),
            client_token: settings.get("clientToken").and_then(as_str).unwrap_or("").trim().to_string(),
        },
        protocols: SUPPORTED_PROTOCOLS
            .iter()
            .map(|(id, name)| {
                let matched = raw_protocols
                    .iter()
                    .find(|item| is_plain_object(item) && item.get("id").and_then(as_str) == Some(*id));
                build_protocol(matched, id, name, &mut issues)
            })
            .collect(),
    };

    for item in &raw_protocols {
        let id = if is_plain_object(item) {
            item.get("id").cloned().unwrap_or(Value::Null)
        } else {
            item.clone()
        };
        let known = id.as_str().map(|text| SUPPORTED_PROTOCOLS.iter().any(|(id, _)| *id == text)).unwrap_or(false);
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
        let base_dir = dir.unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".ccproxy"));
        let config_path = base_dir.join("app.json");
        let temp_path = base_dir.join("app.json.tmp");
        Self { base_dir, config_path, temp_path }
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

        let result = validate_config(&raw);
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

