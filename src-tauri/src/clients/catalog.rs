// codex 的模型清单（<codex 目录>/models.json）：期望条目的推导、与清单的比对、定点改写。
// 这份文件是整份替换 codex 内置清单的，里面可能有别的来源的条目：归 CCProxy 管的
// 只有 slug 等于 OpenAI 协议某条映射的 clientModel 的那些，其余条目一个字节都不碰。

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::clients::json::{
    detect_indent_unit, finish, indent_at, plan_members_at, read_json_array, read_json_object, Edit, JsonEdit, JsonObject,
};
use crate::config::{Provider, Protocol};

pub const CATALOG_FILE: &str = "models.json";
pub const CATALOG_KEY: &str = "model_catalog_json";

// codex 加载清单时逐条硬性要求的键（拿 codex debug models 逐个删键试出来的），
// 少一个整份文件都加载失败；条目还必须有 base_instructions 或
// model_messages.instructions_template 之一，否则语义校验也过不去
pub const REQUIRED_KEYS: [&str; 10] = [
    "slug",
    "display_name",
    "supported_reasoning_levels",
    "shell_type",
    "visibility",
    "supported_in_api",
    "priority",
    "support_verbosity",
    "truncation_policy",
    "experimental_supported_tools",
];
const INSTRUCTIONS_KEYS: [&str; 2] = ["base_instructions", "model_messages"];

const BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.";

// 新建条目的兜底字段：清单里没有可抄的模板时，用这份最小集拼一个 codex 能接受的条目
fn new_entry_default(key: &str) -> Option<Value> {
    match key {
        "shell_type" => Some(json!("shell_command")),
        "visibility" => Some(json!("list")),
        "supported_in_api" => Some(json!(true)),
        "priority" => Some(json!(1000)),
        "support_verbosity" => Some(json!(false)),
        "truncation_policy" => Some(json!({ "mode": "bytes", "limit": 10000 })),
        "experimental_supported_tools" => Some(json!([])),
        "base_instructions" => Some(json!(BASE_INSTRUCTIONS)),
        _ => None,
    }
}

// CCProxy 里勾了推理、清单里却没有档位可抄时用这组（与 codex 自带清单的写法一致）
fn default_reasoning_levels() -> Value {
    json!([
        { "effort": "none", "description": "Disable thinking" },
        { "effort": "low", "description": "Fast responses with lighter reasoning" },
        { "effort": "high", "description": "Enabled high reasoning" },
        { "effort": "max", "description": "Maximum reasoning depth for the hardest problems" }
    ])
}

const DEFAULT_REASONING_LEVEL: &str = "high";

pub fn catalog_file_path(dir_path: &str) -> String {
    crate::clients::dirs::resolve_path(dir_path, CATALOG_FILE)
}

// 文件不存在时从零起一份的骨架，后面照常走定点追加
pub fn empty_catalog_text() -> String {
    "{\n  \"models\": []\n}\n".to_string()
}

#[derive(Debug, Clone)]
pub struct ExpectedEntry {
    pub name: String,
    pub context_window: Option<u64>,
    pub max_context_window: Option<u64>,
    pub input_modalities: Option<Vec<String>>,
    pub reasoning: bool,
}

// OpenAI 协议下「该出现在清单里」的条目：启用的映射（clientModel 不含 *——通配和兜底
// 是路由规则不是模型），名字用 clientModel，能力取映射指向的那个真实模型的配置
// providers 是配置顶层的全局供应商池：映射只存 providerId，供应商本体不再挂在协议下面
pub fn expected_catalog_entries(protocol: Option<&Protocol>, providers: &[Provider]) -> Vec<ExpectedEntry> {
    // 协议整体停用时代理根本不服务它，清单一条都不归我们管
    let Some(protocol) = protocol else {
        return Vec::new();
    };
    if !protocol.enabled {
        return Vec::new();
    }

    let mut entries: Vec<ExpectedEntry> = Vec::new();
    for mapping in &protocol.mappings {
        if !mapping.enabled {
            continue;
        }
        if mapping.client_model.is_empty() || mapping.client_model.contains('*') {
            continue;
        }
        // 手编 app.json 能绕过界面对重名的拦截：同一个名字两条映射会生成两份期望，
        // 对同一条目改两遍会把数值写坏，只认第一条
        if entries.iter().any(|entry| entry.name == mapping.client_model) {
            continue;
        }
        let Some(provider) = providers.iter().find(|item| item.id == mapping.provider_id && item.enabled)
        else {
            continue;
        };
        let model = provider.models.iter().find(|item| item.name == mapping.real_model);
        entries.push(ExpectedEntry {
            name: mapping.client_model.clone(),
            context_window: model.and_then(|item| item.context_window),
            max_context_window: model.and_then(|item| item.max_context_window),
            input_modalities: model.and_then(|item| item.input_modalities.clone()),
            reasoning: model.and_then(|item| item.reasoning).unwrap_or(false),
        });
    }
    entries
}

// ============================================================================ 检查

fn read_catalog(json_text: &str) -> Result<Vec<Value>, String> {
    let parsed: Value = serde_json::from_str(json_text).map_err(|error| format!("模型清单不是合法 JSON：{error}"))?;
    if !parsed.is_object() {
        return Err("模型清单的根节点不是对象".to_string());
    }
    match parsed.get("models").and_then(Value::as_array) {
        Some(models) => Ok(models.clone()),
        None => Err("模型清单的 models 不是数组".to_string()),
    }
}

fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(js_string).collect::<Vec<String>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

// JS 的 String(value)：字符串原样、数组 join('/')、缺键是「没配」
fn describe(value: Option<&Value>) -> String {
    match value {
        None => "没配".to_string(),
        Some(Value::Array(items)) => items.iter().map(js_string).collect::<Vec<String>>().join("/"),
        Some(other) => js_string(other),
    }
}

// 一条归我们管的条目与期望的差：只比 CCProxy 配了的字段，推理比的是「有没有档位」
fn entry_problems(entry: &Value, want: &ExpectedEntry) -> Vec<String> {
    let mut problems = Vec::new();
    let slug = &want.name;
    let object = entry.as_object();
    let field = |key: &str| object.and_then(|map| map.get(key));
    let has = |key: &str| field(key).is_some();

    let missing: Vec<&str> = REQUIRED_KEYS.iter().copied().filter(|key| !has(key)).collect();
    if !missing.is_empty() {
        problems.push(format!("模型清单里 {slug} 缺必需字段 {}，codex 会加载失败", missing.join("、")));
    }
    if !INSTRUCTIONS_KEYS.iter().any(|key| has(key)) {
        problems.push(format!("模型清单里 {slug} 缺 base_instructions，codex 会加载失败"));
    }
    if let Some(want_window) = want.context_window {
        if field("context_window").and_then(Value::as_u64) != Some(want_window) {
            problems.push(format!(
                "模型清单里 {slug} 的上下文窗口是 {}，CCProxy 里配的是 {want_window}",
                describe(field("context_window"))
            ));
        }
    }
    if let Some(want_window) = want.max_context_window {
        if field("max_context_window").and_then(Value::as_u64) != Some(want_window) {
            problems.push(format!(
                "模型清单里 {slug} 的最大上下文窗口是 {}，CCProxy 里配的是 {want_window}",
                describe(field("max_context_window"))
            ));
        }
    }
    if let Some(modalities) = &want.input_modalities {
        let actual = field("input_modalities");
        let matches = actual
            .and_then(Value::as_array)
            .map(|items| {
                items.len() == modalities.len()
                    && items
                        .iter()
                        .zip(modalities.iter())
                        .all(|(item, want)| item.as_str() == Some(want.as_str()))
            })
            .unwrap_or(false);
        if !matches {
            problems.push(format!(
                "模型清单里 {slug} 的输入模态是 {}，CCProxy 里配的是 {}",
                describe(actual),
                modalities.join("/")
            ));
        }
    }
    let levels = field("supported_reasoning_levels")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0);
    if want.reasoning && levels == 0 {
        problems.push(format!("模型清单里 {slug} 没有推理档位，CCProxy 里勾了推理"));
    }
    if !want.reasoning && levels > 0 {
        problems.push(format!("模型清单里 {slug} 配了推理档位，CCProxy 里没勾推理"));
    }
    problems
}

pub struct CatalogInspection {
    pub status: String,
    pub problems: Vec<String>,
}

// 逐条比对：清单里缺我们的模型、条目能力与 CCProxy 配置不一致，都要报。
// 别的来源的条目不比能力也不改，但缺必需键会让整份清单加载失败，得说一声让人自己处理
pub fn inspect_catalog(json_text: &str, expected: &[ExpectedEntry]) -> CatalogInspection {
    let models = match read_catalog(json_text) {
        Ok(models) => models,
        Err(error) => {
            return CatalogInspection {
                status: "unreadable".to_string(),
                problems: vec![error],
            }
        }
    };

    let mut problems: Vec<String> = Vec::new();
    let mut seen: Vec<(String, usize)> = Vec::new();
    for (index, entry) in models.iter().enumerate() {
        let Some(object) = entry.as_object() else {
            problems.push(format!("模型清单第 {} 条不是对象，codex 加载这份清单会失败", index + 1));
            continue;
        };
        let slug = object.get("slug").and_then(Value::as_str).unwrap_or("");
        if let Some(want) = expected.iter().find(|item| item.name == slug) {
            match seen.iter_mut().find(|(name, _)| name == slug) {
                Some((_, count)) => *count += 1,
                None => seen.push((slug.to_string(), 1)),
            }
            problems.extend(entry_problems(entry, want));
            continue;
        }
        let mut missing: Vec<&str> = REQUIRED_KEYS.iter().copied().filter(|key| !object.contains_key(*key)).collect();
        if !INSTRUCTIONS_KEYS.iter().any(|key| object.contains_key(*key)) {
            missing.push("base_instructions");
        }
        if !missing.is_empty() {
            let label = if slug.is_empty() { format!("第 {} 条", index + 1) } else { slug.to_string() };
            problems.push(format!(
                "模型清单里 {label} 缺必需字段 {}，codex 加载这份清单会失败；这条不归 CCProxy 管，请自行补齐",
                missing.join("、")
            ));
        }
    }
    for want in expected {
        match seen.iter().find(|(name, _)| name == &want.name) {
            None => problems.push(format!("模型清单里没有 {}（OpenAI 协议里启用了这条映射）", want.name)),
            Some((_, count)) if *count > 1 => {
                problems.push(format!("模型清单里 {} 出现了 {count} 次，codex 只认第一条", want.name))
            }
            _ => {}
        }
    }

    CatalogInspection {
        status: if problems.is_empty() { "ok".to_string() } else { "mismatch".to_string() },
        problems,
    }
}

// ============================================================================ 改写

fn reasoning_levels_from(entry: &Value) -> Value {
    match entry.get("supported_reasoning_levels").and_then(Value::as_array) {
        Some(items) if !items.is_empty() => Value::Array(items.clone()),
        _ => default_reasoning_levels(),
    }
}

// 已有条目要设成的键值：只含 CCProxy 配了的字段和补缺的必需键——没配的字段不写，
// 清单里已有的值可能是用户自己调的，不动
fn entry_values(want: &ExpectedEntry, current: &Value) -> Vec<(String, Value)> {
    let mut values: Vec<(String, Value)> = Vec::new();
    let has = |key: &str| current.get(key).is_some();

    if let Some(window) = want.context_window {
        values.push(("context_window".to_string(), json!(window)));
    }
    if let Some(window) = want.max_context_window {
        values.push(("max_context_window".to_string(), json!(window)));
    }
    if let Some(modalities) = &want.input_modalities {
        values.push(("input_modalities".to_string(), json!(modalities)));
    }
    if want.reasoning {
        values.push(("supported_reasoning_levels".to_string(), reasoning_levels_from(current)));
        if !has("default_reasoning_level") {
            values.push(("default_reasoning_level".to_string(), json!(DEFAULT_REASONING_LEVEL)));
        }
    } else {
        values.push(("supported_reasoning_levels".to_string(), json!([])));
    }
    for key in REQUIRED_KEYS {
        // slug / display_name 没有兜底值：写进去只会得到非法 JSON，跳过
        if !has(key) {
            if let Some(default) = new_entry_default(key) {
                values.push((key.to_string(), default));
            }
        }
    }
    if !INSTRUCTIONS_KEYS.iter().any(|key| has(key)) {
        values.push(("base_instructions".to_string(), json!(BASE_INSTRUCTIONS)));
    }
    values
}

// 新建条目：抄模板的字段与排序风格（保留 base_instructions、priority 这些我们不在意的），
// 换掉身份与能力字段，缺的必需键用兜底值补
fn build_new_entry(want: &ExpectedEntry, template: Option<&Value>) -> Value {
    let mut entry: Map<String, Value> = template.and_then(Value::as_object).cloned().unwrap_or_default();
    entry.insert("slug".to_string(), json!(want.name));
    entry.insert("display_name".to_string(), json!(want.name));
    entry.insert("description".to_string(), json!(want.name));
    if let Some(window) = want.context_window {
        entry.insert("context_window".to_string(), json!(window));
    }
    if let Some(window) = want.max_context_window {
        entry.insert("max_context_window".to_string(), json!(window));
    }
    if let Some(modalities) = &want.input_modalities {
        entry.insert("input_modalities".to_string(), json!(modalities));
    }
    let levels = if want.reasoning {
        reasoning_levels_from(template.unwrap_or(&Value::Null))
    } else {
        json!([])
    };
    entry.insert("supported_reasoning_levels".to_string(), levels);
    if want.reasoning && !entry.contains_key("default_reasoning_level") {
        entry.insert("default_reasoning_level".to_string(), json!(DEFAULT_REASONING_LEVEL));
    }
    for key in REQUIRED_KEYS {
        if !entry.contains_key(key) {
            if let Some(default) = new_entry_default(key) {
                entry.insert(key.to_string(), default);
            }
        }
    }
    if !INSTRUCTIONS_KEYS.iter().any(|key| entry.contains_key(*key)) {
        entry.insert("base_instructions".to_string(), json!(BASE_INSTRUCTIONS));
    }
    Value::Object(entry)
}

fn to_string_pretty_with(value: &Value, unit: &str) -> String {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    if value.serialize(&mut serializer).is_err() {
        return String::new();
    }
    String::from_utf8(buffer).unwrap_or_default()
}

// 多行块整体补一层缩进：第一行由插入点的逗号换行语法带着，后面的行自己缩
fn indent_block(block: &str, indent: &str) -> String {
    block
        .split('\n')
        .enumerate()
        .map(|(index, line)| if index == 0 { line.to_string() } else { format!("{indent}{line}") })
        .collect::<Vec<String>>()
        .join("\n")
}

struct Item {
    start: usize,
    end: usize,
    object: Option<JsonObject>,
}

// 定点改写：缺的条目按模板补进数组末尾，已有的条目只改我们管的字段，
// 与我们重名的多余条目删掉，别的条目与全文格式一个字节不动
pub fn rewrite_catalog(json_text: &str, expected: &[ExpectedEntry]) -> Result<JsonEdit, String> {
    let text = json_text.to_string();
    read_catalog(&text).map_err(|error| format!("{error}，为免写坏已放弃"))?;

    let Some(root_start) = text.find('{') else {
        return Err("模型清单结构看不懂，为免写坏已放弃".to_string());
    };
    let Some(root) = read_json_object(&text, root_start) else {
        return Err("模型清单结构看不懂，为免写坏已放弃".to_string());
    };
    let Some(models_index) = root.members.iter().position(|member| member.name == "models") else {
        return Err("模型清单结构看不懂，为免写坏已放弃".to_string());
    };
    let models_start = root.members[models_index].value_start;
    let Some(array) = read_json_array(&text, models_start) else {
        return Err("模型清单结构看不懂，为免写坏已放弃".to_string());
    };

    let unit = detect_indent_unit(&text);
    let items: Vec<Item> = array
        .items
        .iter()
        .map(|span| Item {
            start: span.0,
            end: span.1,
            object: read_json_object(&text, span.0),
        })
        .collect();
    // 模板取第一个对象条目：新建条目抄它的字段与排序，跟清单自己的风格走
    let template = items
        .iter()
        .find(|item| item.object.is_some())
        .and_then(|item| serde_json::from_str::<Value>(&text[item.start..item.end]).ok());

    let mut edits: Vec<Edit> = Vec::new();
    let mut appended: Vec<Value> = Vec::new();
    for want in expected {
        let matches: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let Some(object) = &item.object else { return false };
                let Some(member) = object.members.iter().find(|member| member.name == "slug") else {
                    return false;
                };
                serde_json::from_str::<Value>(&text[member.value_start..member.value_end])
                    .map(|value| value.as_str() == Some(want.name.as_str()))
                    .unwrap_or(false)
            })
            .map(|(index, _)| index)
            .collect();

        if matches.is_empty() {
            appended.push(build_new_entry(want, template.as_ref()));
            continue;
        }

        let hit = &items[matches[0]];
        let object = hit.object.as_ref().expect("命中的条目一定有对象");
        let current: Value = serde_json::from_str(&text[hit.start..hit.end]).unwrap_or(Value::Null);
        plan_members_at(&text, object, hit.start, &entry_values(want, &current), &unit, &mut edits);

        // 重名的多余条目删掉：codex 只认第一条，留着它检查永远收敛不到 ok。
        // 删除区间从上一个元素的尾到这个元素的尾，把中间那个逗号一并带走
        let extras: Vec<usize> = matches.iter().skip(1).copied().collect();
        for duplicate in extras {
            edits.push(Edit {
                start: items[duplicate - 1].end,
                end: items[duplicate].end,
                text: String::new(),
            });
        }
    }

    if !appended.is_empty() {
        let block = |indent: &str| {
            appended
                .iter()
                .map(|entry| indent_block(&to_string_pretty_with(entry, &unit), indent))
                .collect::<Vec<String>>()
                .join(&format!(",\n{indent}"))
        };
        if items.is_empty() {
            let close_indent = indent_at(&text, array.close_index);
            let item_indent = format!("{close_indent}{unit}");
            edits.push(Edit {
                start: models_start + 1,
                end: models_start + 1,
                text: format!("\n{}\n{close_indent}", block(&item_indent)),
            });
        } else {
            // 追加在末尾，照第一个条目的缩进对齐；几个新条目合成一次插入，不在同一位置打架
            let item_indent = indent_at(&text, items[0].start);
            let anchor = items[items.len() - 1].end;
            edits.push(Edit {
                start: anchor,
                end: anchor,
                text: format!(",\n{}", block(&item_indent)),
            });
        }
    }

    finish(&text, edits)
}

