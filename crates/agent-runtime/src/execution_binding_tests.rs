use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::cache::TurnHit;
use agent_core::{ExecutionProfileId, SessionConfig, TokenUsage};
use agent_providers::deepseek::DeepSeek;
use agent_providers::kimi::Kimi;
use agent_tools::ToolExecutor;
use agent_transport::Client;

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
