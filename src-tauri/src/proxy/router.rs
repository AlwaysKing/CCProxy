// 映射查表：在指定协议的映射表里为客户端模型名找到目标供应商与真实模型名。
// 优先级：精确匹配 > 通配匹配（前缀最长者优先）> 兜底规则（clientModel 为 *）。
// 启停规则：协议停用报 503；供应商或单条映射停用的条目视为不可用直接跳过。

use crate::config::{AppConfig, Mapping};

#[derive(Debug, Clone)]
pub struct Route {
    pub protocol_id: String,
    pub protocol_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub token: String,
    // 供应商声明的上游线格式：auto | anthropic | openai_chat | openai_responses
    pub api_format: String,
    pub mapping: Mapping,
    pub match_kind: String,
    pub real_model: String,
}

#[derive(Debug, Clone)]
pub struct RouteError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

fn is_wildcard_pattern(pattern: &str) -> bool {
    pattern.len() > 1 && pattern.ends_with('*')
}

// 单条规则能否匹配客户端模型名（精确 / 通配 / 兜底三种形态）
pub fn match_mapping(mapping: &Mapping, client_model: &str) -> bool {
    let pattern = mapping.client_model.as_str();
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    if pattern == client_model {
        return true;
    }
    if is_wildcard_pattern(pattern) {
        return client_model.starts_with(&pattern[..pattern.len() - 1]);
    }
    false
}

pub fn resolve_routing(config: &AppConfig, protocol_id: &str, client_model: &str) -> Result<Route, RouteError> {
    let Some(protocol) = config.protocol(protocol_id) else {
        return Err(RouteError {
            status: 404,
            code: "protocol_not_found".to_string(),
            message: format!("配置中不存在协议 {protocol_id}"),
        });
    };
    if !protocol.enabled {
        return Err(RouteError {
            status: 503,
            code: "protocol_disabled".to_string(),
            message: format!("协议 {}（{}）已停用", protocol.name, protocol.id),
        });
    }

    // 只保留可用条目：映射启用、供应商存在且启用。
    // 供应商现在挂在配置顶层（三个协议共用），所以从 config.providers 里找而不是协议下
    let mut usable: Vec<(&Mapping, &crate::config::Provider)> = Vec::new();
    for mapping in &protocol.mappings {
        if !mapping.enabled {
            continue;
        }
        let Some(provider) = config.providers.iter().find(|item| item.id == mapping.provider_id) else {
            continue;
        };
        if !provider.enabled {
            continue;
        }
        usable.push((mapping, provider));
    }

    // 三轮各记一个 matchKind：请求日志要写「命中规则」，而「是哪一轮命中的」只有这里知道。
    // 第一轮：精确匹配
    let mut hit = usable.iter().find(|(mapping, _)| mapping.client_model == client_model);
    let mut match_kind = if hit.is_some() { Some("exact") } else { None };
    // 第二轮：通配，前缀最长者优先
    if hit.is_none() {
        let mut wildcards: Vec<&(&Mapping, &crate::config::Provider)> = usable
            .iter()
            .filter(|(mapping, _)| is_wildcard_pattern(&mapping.client_model) && match_mapping(mapping, client_model))
            .collect();
        wildcards.sort_by(|a, b| b.0.client_model.len().cmp(&a.0.client_model.len()));
        hit = wildcards.first().copied();
        match_kind = if hit.is_some() { Some("wildcard") } else { None };
    }
    // 第三轮：兜底
    if hit.is_none() {
        hit = usable.iter().find(|(mapping, _)| mapping.client_model == "*");
        match_kind = if hit.is_some() { Some("fallback") } else { None };
    }

    let Some((mapping, provider)) = hit else {
        let label = if client_model.is_empty() { "(空)" } else { client_model };
        return Err(RouteError {
            status: 404,
            code: "mapping_not_found".to_string(),
            message: format!("模型 {label} 未在 {} 协议映射表中配置", protocol.name),
        });
    };

    Ok(Route {
        protocol_id: protocol_id.to_string(),
        protocol_name: protocol.name.clone(),
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        token: provider.token.clone(),
        api_format: provider.api_format.clone(),
        mapping: (*mapping).clone(),
        match_kind: match_kind.unwrap_or("exact").to_string(),
        real_model: mapping.real_model.clone(),
    })
}

