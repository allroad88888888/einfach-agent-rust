use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::cache::TurnHit;
use agent_core::{ExecutionProfileId, SessionConfig, TokenUsage};
use agent_providers::deepseek::DeepSeek;
use agent_providers::kimi::Kimi;
use agent_tools::ToolExecutor;
use agent_transport::{Client, RootConfig, default_provider};

use super::*;
use crate::tool_table::ToolTable;

fn config(model: &str) -> SessionConfig {
    SessionConfig {
        model: Arc::from(model),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

fn build() -> RunnerCtx {
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "deepseek-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        config("deepseek-v4-pro"),
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

fn kimi_binding(model: &str, key: &str) -> ExecutionBinding {
    ExecutionBinding::new(
        Arc::new(Kimi),
        Arc::new(Client::new()),
        "https://api.moonshot.cn/v1/chat/completions".to_string(),
        key.to_string(),
        config(model),
    )
}

#[test]
fn selects_named_binding_only_for_matching_profile() {
    let vision = ExecutionProfileId::new("vision");
    let ctx = build().with_execution_bindings(BTreeMap::from([(
        vision.clone(),
        kimi_binding("kimi-vision", "vision-key"),
    )]));

    let named = ctx.execution_binding_for(Some(&vision)).unwrap().binding;
    let default = ctx.execution_binding_for(None).unwrap().binding;

    assert_eq!(&*named.session_config.model, "kimi-vision");
    assert_eq!(named.api_key, "vision-key");
    assert_eq!(&*default.session_config.model, "deepseek-v4-pro");
}

#[test]
fn named_binding_survives_default_switch() {
    let vision = ExecutionProfileId::new("vision");
    let mut ctx = build().with_execution_bindings(BTreeMap::from([(
        vision.clone(),
        kimi_binding("kimi-vision", "vision-key"),
    )]));

    ctx.switch_provider(
        Arc::new(Kimi),
        "https://api.moonshot.cn/v1/chat/completions".to_string(),
        "default-kimi-key".to_string(),
        Arc::from("kimi-default"),
    );

    let named = ctx.execution_binding_for(Some(&vision)).unwrap().binding;
    assert_eq!(&*named.session_config.model, "kimi-vision");
    assert_eq!(named.api_key, "vision-key");
}

#[test]
fn guard_histories_are_isolated_by_binding_scope() {
    let vision = ExecutionProfileId::new("vision");
    let hit = TurnHit::from_usage(&TokenUsage {
        prompt: 100,
        completion: 10,
        cached: Some(64),
    });
    let mut ctx = build().with_execution_bindings(BTreeMap::from([(
        vision.clone(),
        kimi_binding("kimi-vision", "vision-key"),
    )]));
    let default_scope = ctx.execution_binding_for(None).unwrap().guard_scope;
    let named_scope = ctx
        .execution_binding_for(Some(&vision))
        .unwrap()
        .guard_scope;

    ctx.guard_history_for(default_scope).push(hit.clone());
    ctx.guard_history_for(named_scope).push(hit);

    assert_eq!(ctx.guard_histories.get(&default_scope).unwrap().len(), 1);
    assert_eq!(ctx.guard_histories.get(&named_scope).unwrap().len(), 1);
    assert_eq!(ctx.guard_histories.len(), 2);
}

/// 编的假 key，不是真凭据——仓规矩：测试夹具永远不许放真 key。
const FAKE_RUNTIME_KEY: &str = "sk-fake-runtime-binding-test-not-real-999";

/// **114d 的主证据**：[`ExecutionBinding::from_provider_config`] 是「配置 →
/// 运行时 binding」唯一一条装配路径。这里喂给它两份来源不同的
/// `agent_transport::ProviderConfig`——一份用 `serde_json::from_str` 反序列化
/// （模拟宿主跨 wasm-bindgen 边界传来的「已解析配置」；native 侧的等价物是
/// `agent_transport::config` 用 `toml::from_str` 解析出的同一个类型，
/// agent-runtime 不依赖 `toml`，这里换两边都有的 `serde_json` 达到同样的
/// 「反序列化产出」效果），另一份用 `ProviderConfig::from_host` 直接构造
/// （宿主注入）——两者喂进同一个函数必须产出逐字段相同的 binding。
#[test]
fn execution_binding_from_provider_config_converges_regardless_of_config_origin() {
    let parsed_json = format!(
        r#"{{
            "providers": {{
                "deepseek": {{
                    "api_key": "{FAKE_RUNTIME_KEY}",
                    "base_url": "https://api.deepseek.com",
                    "model": "deepseek-v4-pro"
                }}
            }},
            "default": {{ "provider": "deepseek" }}
        }}"#
    );
    let parsed: RootConfig = serde_json::from_str(&parsed_json).unwrap();
    let parsed_provider = default_provider(&parsed).unwrap();

    let injected_provider = ProviderConfig::from_host(
        "https://api.deepseek.com".to_string(),
        "deepseek-v4-pro".to_string(),
        FAKE_RUNTIME_KEY.to_string(),
    );

    let from_parsed = ExecutionBinding::from_provider_config(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        parsed_provider,
        config("deepseek-v4-pro"),
    )
    .expect("resolve_key 应该拿到 FAKE_RUNTIME_KEY");
    let from_injected = ExecutionBinding::from_provider_config(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        &injected_provider,
        config("deepseek-v4-pro"),
    )
    .expect("resolve_key 应该拿到 FAKE_RUNTIME_KEY");

    assert_eq!(from_parsed.endpoint, from_injected.endpoint);
    assert_eq!(from_parsed.api_key, from_injected.api_key);
    assert_eq!(from_parsed.api_key, FAKE_RUNTIME_KEY);
    assert_eq!(
        &*from_parsed.session_config.model,
        &*from_injected.session_config.model
    );
}

#[test]
fn execution_binding_from_provider_config_is_none_when_key_is_missing() {
    let no_key = ProviderConfig::from_host(
        "https://api.deepseek.com".to_string(),
        "deepseek-v4-pro".to_string(),
        String::new(),
    );
    let binding = ExecutionBinding::from_provider_config(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        &no_key,
        config("deepseek-v4-pro"),
    );
    assert!(binding.is_none());
}
