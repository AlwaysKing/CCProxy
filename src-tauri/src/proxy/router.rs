// 映射查表：在指定协议的映射表里为客户端模型名找到目标供应商与真实模型名。
// 匹配规则：按映射表顺序从上到下逐条匹配，第一条命中的规则生效——列表顺序即优先级，
// 前端拖拽排序就是在调整它。
// 启停规则：协议停用报 503；供应商或单条映射停用的条目视为不可用直接跳过。

use crate::config::{AppConfig, Mapping, ModelGroup, Provider};

#[derive(Debug, Clone)]
pub struct Route {
    pub protocol_id: String,
    pub protocol_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model_group_id: String,
    pub base_url: String,
    pub token: String,
    // 实际发送给上游的线格式：anthropic | openai_chat | openai_responses
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

pub fn resolve_routing(
    config: &AppConfig,
    protocol_id: &str,
    client_model: &str,
) -> Result<Route, RouteError> {
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
    let mut usable: Vec<(&Mapping, &Provider, &ModelGroup)> = Vec::new();
    for mapping in &protocol.mappings {
        if !mapping.enabled {
            continue;
        }
        let Some(provider) = config
            .providers
            .iter()
            .find(|item| item.id == mapping.provider_id)
        else {
            continue;
        };
        if !provider.enabled {
            continue;
        }
        let Some(group) = provider
            .model_groups
            .iter()
            .find(|item| item.id == mapping.model_group_id)
        else {
            continue;
        };
        usable.push((mapping, provider, group));
    }

    // 按映射表顺序匹配，首条命中生效。matchKind 由命中的规则形态推导，供请求日志展示
    let hit = usable
        .iter()
        .find(|(mapping, _, _)| match_mapping(mapping, client_model));
    let match_kind = hit.map(|(mapping, _, _)| {
        let pattern = mapping.client_model.as_str();
        if pattern == "*" {
            "fallback"
        } else if is_wildcard_pattern(pattern) {
            "wildcard"
        } else {
            "exact"
        }
    });

    let Some((mapping, provider, group)) = hit else {
        let label = if client_model.is_empty() {
            "(空)"
        } else {
            client_model
        };
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
        model_group_id: group.id.clone(),
        base_url: provider.base_url.clone(),
        token: provider.token.clone(),
        api_format: mapping.upstream_protocol.clone(),
        mapping: (*mapping).clone(),
        match_kind: match_kind.unwrap_or("exact").to_string(),
        real_model: mapping.real_model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Model, Protocol, Settings, CONFIG_VERSION, DEFAULT_PORT};

    fn mapping(id: &str, client_model: &str, real_model: &str) -> Mapping {
        Mapping {
            id: id.to_string(),
            enabled: true,
            client_model: client_model.to_string(),
            provider_id: "p1".to_string(),
            model_group_id: "g1".to_string(),
            upstream_protocol: "anthropic".to_string(),
            real_model: real_model.to_string(),
        }
    }

    fn config_with(mappings: Vec<Mapping>) -> AppConfig {
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
            providers: vec![Provider {
                id: "p1".to_string(),
                name: "测试供应商".to_string(),
                enabled: true,
                base_url: "https://example.com".to_string(),
                token: "sk-test".to_string(),
                model_groups: vec![ModelGroup {
                    id: "g1".to_string(),
                    api_format: "anthropic".to_string(),
                    models: vec![Model {
                        name: "real-a".to_string(),
                        context_window: None,
                        max_context_window: None,
                        input_modalities: None,
                        reasoning: None,
                    }],
                }],
            }],
            protocols: vec![Protocol {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                enabled: true,
                mappings,
            }],
        }
    }

    fn resolve(config: &AppConfig, client_model: &str) -> (String, String) {
        let route = resolve_routing(config, "anthropic", client_model).expect("应当命中映射");
        (route.real_model, route.match_kind)
    }

    #[test]
    fn wildcard_above_exact_wins_by_list_order() {
        let config = config_with(vec![
            mapping("m1", "claude-*", "real-wildcard"),
            mapping("m2", "claude-3", "real-exact"),
        ]);
        assert_eq!(
            resolve(&config, "claude-3"),
            ("real-wildcard".to_string(), "wildcard".to_string())
        );
    }

    #[test]
    fn exact_above_wildcard_wins_by_list_order() {
        let config = config_with(vec![
            mapping("m1", "claude-3", "real-exact"),
            mapping("m2", "claude-*", "real-wildcard"),
        ]);
        assert_eq!(
            resolve(&config, "claude-3"),
            ("real-exact".to_string(), "exact".to_string())
        );
        assert_eq!(
            resolve(&config, "claude-4"),
            ("real-wildcard".to_string(), "wildcard".to_string())
        );
    }

    #[test]
    fn fallback_above_others_wins_by_list_order() {
        let config = config_with(vec![
            mapping("m1", "*", "real-fallback"),
            mapping("m2", "claude-3", "real-exact"),
        ]);
        assert_eq!(
            resolve(&config, "claude-3"),
            ("real-fallback".to_string(), "fallback".to_string())
        );
    }

    #[test]
    fn disabled_mapping_is_skipped() {
        let mut disabled = mapping("m1", "claude-*", "real-wildcard");
        disabled.enabled = false;
        let config = config_with(vec![disabled, mapping("m2", "claude-3", "real-exact")]);
        assert_eq!(
            resolve(&config, "claude-3"),
            ("real-exact".to_string(), "exact".to_string())
        );
    }
}
