// 客户端配置检查：目录里的配置文件是不是指向 CCProxy。
// claude 读 settings.json 的 env.ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN；
// codex 读 config.toml 里 model_provider 指向的那个 [model_providers.<id>] 块的
// base_url / experimental_bearer_token。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use url::Url;

use crate::clients::catalog::{catalog_file_path, inspect_catalog, CATALOG_FILE, CATALOG_KEY, ExpectedEntry};
use crate::clients::dirs::resolve_single;
use crate::clients::toml::{scan_toml_lines, LineKind};

// 认作「本机」的主机名。地址必须搭配端口一起判：别的本地代理也挂在本机，
// 只看主机名会把它的配置当成我们的
const LOOPBACK_HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "::1", "[::1]"];
// provider 块的表名前缀（apply 那边写的是同一个，两侧靠「写完再检查一次」互通）
const PROVIDER_SECTION_PREFIX: &str = "model_providers.";

pub struct InspectContext {
    pub port: u16,
    pub token: String,
    pub catalog: Vec<ExpectedEntry>,
    pub address: String,
}

impl InspectContext {
    pub fn new(port: u16, token: &str, catalog: Vec<ExpectedEntry>) -> Self {
        Self {
            port,
            token: token.to_string(),
            catalog,
            address: format!("http://127.0.0.1:{port}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InspectResult {
    pub status: String,
    pub target: String,
    pub problems: Vec<String>,
}

// 只比主机与端口，不比路径：base_url 带不带 /v1 转发都能对上（上游拼接会去重）
pub fn is_ccproxy_address(raw_url: &str, port: u16) -> bool {
    let Ok(url) = Url::parse(raw_url.trim()) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if !LOOPBACK_HOSTS.contains(&host) {
        return false;
    }
    url.port_or_known_default() == Some(port)
}

fn collect_address_problem(base_url: &str, context: &InspectContext, label: &str) -> String {
    if base_url.is_empty() {
        return format!("没有配置 {label}");
    }
    if !is_ccproxy_address(base_url, context.port) {
        return format!("{label} 指向 {base_url}，不是 CCProxy（{}）", context.address);
    }
    String::new()
}

fn collect_token_problem(token: &str, context: &InspectContext) -> String {
    if token.is_empty() {
        return "没有配置 token".to_string();
    }
    if token != context.token {
        return "token 与 CCProxy 的客户端 token 不一致".to_string();
    }
    String::new()
}

struct ClaudeEndpoint {
    base_url: String,
    token: String,
}

fn read_claude_endpoint(dir_path: &str) -> Result<ClaudeEndpoint, String> {
    let settings_path = Path::new(dir_path).join("settings.json");
    let text = fs::read_to_string(&settings_path).map_err(|error| format!("读取 settings.json 失败：{error}"))?;
    let parsed: Value =
        serde_json::from_str(&text).map_err(|error| format!("settings.json 不是合法 JSON：{error}"))?;
    if !parsed.is_object() {
        return Err("settings.json 的根节点不是对象".to_string());
    }
    let env = parsed.get("env").filter(|value| value.is_object());
    Ok(ClaudeEndpoint {
        base_url: env
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        token: env
            .and_then(|env| env.get("ANTHROPIC_AUTH_TOKEN"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    })
}

pub struct CodexProvider {
    pub provider_id: String,
    pub base_url: String,
    pub token: String,
    pub requires_openai_auth: bool,
    pub catalog_path: String,
}

fn read_codex_provider(dir_path: &str) -> Result<CodexProvider, String> {
    let config_path = Path::new(dir_path).join("config.toml");
    let text = fs::read_to_string(&config_path).map_err(|error| format!("读取 config.toml 失败：{error}"))?;

    let scan = scan_toml_lines(&text);
    // 三引号没闭合的文件已经读不动了（codex 自己也解析不了），别硬猜里面是什么
    if scan.unterminated {
        return Err("config.toml 里有多行字符串没闭合（三引号只有一个）".to_string());
    }

    let mut providers: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut provider_id = String::new();
    let mut catalog_path = String::new();
    for line in &scan.lines {
        if line.kind != LineKind::Entry {
            continue;
        }
        if line.section.is_empty() {
            if line.key == "model_provider" {
                provider_id = line.value.clone();
            }
            if line.key == CATALOG_KEY {
                catalog_path = line.value.clone();
            }
            continue;
        }
        // 节名已归一化，[model_providers."ccproxy"] 与 [model_providers.ccproxy] 是同一个
        let Some(id) = line.section.strip_prefix(PROVIDER_SECTION_PREFIX) else {
            continue;
        };
        providers.entry(id.to_string()).or_default().insert(line.key.clone(), line.value.clone());
    }

    if provider_id.is_empty() {
        return Err("config.toml 没有配置 model_provider".to_string());
    }
    let Some(block) = providers.get(&provider_id) else {
        return Err(format!("model_provider 指向 {provider_id}，但配置里没有 [model_providers.{provider_id}] 块"));
    };

    Ok(CodexProvider {
        provider_id,
        base_url: block.get("base_url").cloned().unwrap_or_default(),
        token: block.get("experimental_bearer_token").cloned().unwrap_or_default(),
        requires_openai_auth: block.get("requires_openai_auth").map(String::as_str) == Some("true"),
        catalog_path,
    })
}

fn inspect_claude_dir(dir_path: &str, context: &InspectContext) -> InspectResult {
    let endpoint = match read_claude_endpoint(dir_path) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return InspectResult {
                status: "unreadable".to_string(),
                target: String::new(),
                problems: vec![error],
            }
        }
    };

    let mut problems = Vec::new();
    let address_problem = collect_address_problem(&endpoint.base_url, context, "ANTHROPIC_BASE_URL");
    if !address_problem.is_empty() {
        problems.push(address_problem);
    }
    let token_problem = collect_token_problem(&endpoint.token, context);
    if !token_problem.is_empty() {
        problems.push(format!("ANTHROPIC_AUTH_TOKEN {token_problem}"));
    }
    InspectResult {
        status: if problems.is_empty() { "ok".to_string() } else { "mismatch".to_string() },
        target: endpoint.base_url,
        problems,
    }
}

// 模型清单的检查：指向键在不在、指没指对、清单里的条目与 CCProxy 配置是否一致
fn collect_catalog_problems(dir_path: &str, provider: &CodexProvider, context: &InspectContext, problems: &mut Vec<String>) -> String {
    let expected_path = catalog_file_path(dir_path);
    if provider.catalog_path.is_empty() {
        problems.push(format!("config.toml 没有配置 {CATALOG_KEY}，codex 读不到模型清单"));
    } else if resolve_single(&provider.catalog_path) != expected_path {
        problems.push(format!(
            "{CATALOG_KEY} 指向 {}，不是 {expected_path}",
            provider.catalog_path
        ));
    }

    let file_path = Path::new(dir_path).join(CATALOG_FILE);
    let text = match fs::read_to_string(&file_path) {
        Ok(text) => text,
        Err(error) => {
            // 文件不存在是能「应用」补出来的毛病；别的读失败（权限之类）才是没法判断
            if error.kind() == std::io::ErrorKind::NotFound {
                problems.push(format!("没有 {CATALOG_FILE}，codex 读不到模型清单"));
                return "mismatch".to_string();
            }
            problems.push(format!("读取 {CATALOG_FILE} 失败：{error}"));
            return "unreadable".to_string();
        }
    };
    let inspect = inspect_catalog(&text, &context.catalog);
    problems.extend(inspect.problems);
    inspect.status
}

fn inspect_codex_dir(dir_path: &str, context: &InspectContext) -> InspectResult {
    let provider = match read_codex_provider(dir_path) {
        Ok(provider) => provider,
        Err(error) => {
            return InspectResult {
                status: "unreadable".to_string(),
                target: String::new(),
                problems: vec![error],
            }
        }
    };

    let mut problems = Vec::new();
    let address_problem = collect_address_problem(&provider.base_url, context, "base_url");
    if !address_problem.is_empty() {
        problems.push(address_problem);
    }
    // requires_openai_auth = true 时 codex 送的是 auth.json 里的 ChatGPT 凭据，
    // 不是配置里的 token——这种配置过不了 CCProxy 的入站校验，得指出来
    if provider.requires_openai_auth {
        problems.push("requires_openai_auth 为 true，codex 会用 auth.json 里的凭据而不是这个 token".to_string());
    } else {
        let token_problem = collect_token_problem(&provider.token, context);
        if !token_problem.is_empty() {
            problems.push(format!("experimental_bearer_token {token_problem}"));
        }
    }

    let mut status = if problems.is_empty() { "ok".to_string() } else { "mismatch".to_string() };
    if !context.catalog.is_empty() {
        let catalog_status = collect_catalog_problems(dir_path, &provider, context, &mut problems);
        // 清单读不出来（坏 JSON 之类）是「没法判断」，和能读但内容不对分开报
        if catalog_status == "unreadable" {
            status = "unreadable".to_string();
        } else if catalog_status == "mismatch" && status == "ok" {
            status = "mismatch".to_string();
        }
    }

    InspectResult {
        status,
        target: provider.base_url,
        problems,
    }
}

// 检查一个目录。context 里的 address 由 port 推出，给界面显示「应该指向哪」；
// catalog 是 OpenAI 协议算出的期望清单条目，只有 codex 用
pub fn inspect_client_config(dir_path: &str, kind: &str, context: &InspectContext) -> InspectResult {
    match kind {
        "claude" => inspect_claude_dir(dir_path, context),
        "codex" => inspect_codex_dir(dir_path, context),
        other => InspectResult {
            status: "unreadable".to_string(),
            target: String::new(),
            problems: vec![format!("未知的目录类型：{other}")],
        },
    }
}

