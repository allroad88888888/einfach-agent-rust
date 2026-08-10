//! `RunnerCtx` 的单元测试（红线 9：从 `ctx.rs` 挪出来，源文件只留实现）。
//! `#[path]` 子模块，`super` 仍是 `ctx`，私有字段/方法照样够得着。

use super::*;
use agent_core::TokenUsage;
use agent_core::cache::TurnHit;
use agent_providers::deepseek::DeepSeek;
use agent_providers::kimi::Kimi;
use agent_tools::ToolExecutor;

use crate::tool_table::ToolTable;

fn build(model: &str) -> RunnerCtx {
    let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "deepseek-key".to_string(),
        fs,
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from(model),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

#[test]
fn switch_provider_replaces_adapter_endpoint_key_model_and_clears_guard_window() {
    let mut ctx = build("deepseek-v4-pro");
    let old_scope = ctx.execution_binding_for(None).unwrap().guard_scope;
    ctx.guard_history_for(old_scope)
        .push(TurnHit::from_usage(&TokenUsage {
            prompt: 100,
            completion: 10,
            cached: Some(64),
        }));
    assert!(!ctx.guard_history_for(old_scope).is_empty());

    ctx.switch_provider(
        Arc::new(Kimi),
        "https://api.moonshot.cn/v1/chat/completions".to_string(),
        "kimi-key".to_string(),
        Arc::from("kimi-k3"),
    );

    assert_eq!(
        ctx.default_binding.endpoint,
        "https://api.moonshot.cn/v1/chat/completions"
    );
    assert_eq!(ctx.default_binding.api_key, "kimi-key");
    assert_eq!(&*ctx.default_binding.session_config.model, "kimi-k3");
    let new_scope = ctx.execution_binding_for(None).unwrap().guard_scope;
    assert_ne!(new_scope, old_scope);
    assert!(
        ctx.guard_history_for(new_scope).is_empty(),
        "跨家滚动窗口该清空，不能把 deepseek 的观测带进 kimi 的命中率"
    );
}

/// 014 验收原文点名的断言：切到 kimi 之后，真的 `encode` 一次，产出的
/// body 得是 kimi 的形状——带上新 model 名、不残留旧家的 model 名。只测
/// `switch_provider` 换掉的三个字段（`provider`/`endpoint`/`session_config.
/// model`）互相独立地对不上是不够的：万一 `provider` 换了但
/// `session_config.model` 没跟着换（或者反过来），字段级断言会各自通过，
/// 只有真的 encode 一次才会暴露「adapter 用的是新家，却拿旧家的 model
/// 名去发请求」这种组合错误。
#[test]
fn switch_provider_encode_reflects_the_new_family_not_the_old() {
    let mut ctx = build("deepseek-v4-pro");
    ctx.switch_provider(
        Arc::new(Kimi),
        "https://api.moonshot.cn/v1/chat/completions".to_string(),
        "kimi-key".to_string(),
        Arc::from("kimi-k3"),
    );

    let encoded = ctx
        .default_binding
        .provider
        .encode(&agent_providers::Ingredients {
            system: &[],
            messages: &[],
            tools: &[],
            late_tools: &[],
            late_system: &[],
            config: &ctx.default_binding.session_config,
            intent: agent_core::RequestIntent::Free,
            prev_prefix: None,
        });

    let body = String::from_utf8(encoded.body).unwrap();
    assert!(
        body.contains("kimi-k3"),
        "encode 该带上切换后的 model 名: {body}"
    );
    assert!(
        !body.contains("deepseek-v4-pro"),
        "encode 出的 body 不该残留切换前那家的 model 名: {body}"
    );
}
