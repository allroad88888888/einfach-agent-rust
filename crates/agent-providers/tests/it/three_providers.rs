//! 三家横向对比（issue 023 验收，红线 11）。
//!
//! 独立测试 agent 规则：只依据 `docs/issues/023-three-providers.md`、
//! `probes/PROVIDERS.md`、`docs/INVARIANTS.md` 红线 11/12、`docs/ADAPTER.md`，
//! 以及 `agent-providers` / `agent-core` 的公开签名。**不看任何一家的实现体**
//! （`src/deepseek/`、`src/kimi/`、`src/glm/`、`src/wire.rs`）。
//!
//! 红线 12 的元测试已经在 `invariants_meta.rs` 里，本文件不重复写。

mod support;

use std::sync::Arc;

use agent_core::{RequestIntent, SessionConfig, ToolSpec};
use agent_providers::Provider;
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;

use support::{
    assistant_text, ingredients, schema_order_a, schema_order_b, sys_chunk, tool_spec, user_text,
};

fn config_for(model: &str) -> SessionConfig {
    SessionConfig {
        model: Arc::from(model),
        temperature: Some(0.7),
        max_tokens: Some(4096),
        context_window: Some(128_000),
    }
}

/// 三家横向 1：同一份料（同 system/messages/tools/intent，只有 model 字段随
/// provider 变——那是「跟哪家说话」的必要标识，不是料本身），三家 encode 出来
/// 的 body 字节两两不同。三份骨架都基于 OpenAI 兼容 wire，但 model 名、
/// tool_choice/temperature 的组装差异必须体现在字节上。
#[test]
fn same_ingredients_three_bodies_pairwise_different() {
    let sys = [sys_chunk("base", "you are a helpful agent")];
    let messages = [user_text(1, "read the file"), assistant_text(2, "ok")];
    let tools = [tool_spec(
        "srv:fs/read",
        "read a file",
        serde_json::json!({"type": "object"}),
    )];
    let late_tools: [ToolSpec; 0] = [];

    let cfg_ds = config_for("deepseek-v4-pro");
    let cfg_kimi = config_for("kimi-k3");
    let cfg_glm = config_for("glm-5.2");

    let ing_ds = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &cfg_ds,
        RequestIntent::Free,
        None,
    );
    let ing_kimi = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &cfg_kimi,
        RequestIntent::Free,
        None,
    );
    let ing_glm = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &cfg_glm,
        RequestIntent::Free,
        None,
    );

    let body_ds = DeepSeek.encode(&ing_ds).body;
    let body_kimi = Kimi.encode(&ing_kimi).body;
    let body_glm = Glm.encode(&ing_glm).body;

    assert_ne!(body_ds, body_kimi, "DeepSeek 与 Kimi 的 body 不该撞");
    assert_ne!(body_ds, body_glm, "DeepSeek 与 GLM 的 body 不该撞");
    assert_ne!(body_kimi, body_glm, "Kimi 与 GLM 的 body 不该撞");
}

/// 红线 11：三家**各自**，同一份料两次 encode 逐字节相同——不是只查一份，三家
/// 都要过。schema 用两种不同的 key 插入顺序构造，顺带复证 `serde_json::Map`
/// 是 `BTreeMap`（agent-core 的 `tool.rs` 已经在料单层面证过一遍，这里在三家
/// 的组装产物上再证一遍）。
#[test]
fn each_provider_encode_is_byte_deterministic() {
    let sys = [sys_chunk("base", "you are a helpful agent")];
    let messages = [user_text(1, "read the file"), assistant_text(2, "ok")];
    let tools = [
        tool_spec("srv:fs/read", "read a file", schema_order_a()),
        tool_spec("srv:fs/write", "write a file", schema_order_b()),
    ];
    let late_tools: [ToolSpec; 0] = [];

    let cfg_ds = config_for("deepseek-v4-pro");
    let ing_ds = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &cfg_ds,
        RequestIntent::Free,
        None,
    );
    assert_eq!(
        DeepSeek.encode(&ing_ds).body,
        DeepSeek.encode(&ing_ds).body,
        "DeepSeek 不确定性"
    );

    let cfg_kimi = config_for("kimi-k3");
    let ing_kimi = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &cfg_kimi,
        RequestIntent::Free,
        None,
    );
    assert_eq!(
        Kimi.encode(&ing_kimi).body,
        Kimi.encode(&ing_kimi).body,
        "Kimi 不确定性"
    );

    let cfg_glm = config_for("glm-5.2");
    let ing_glm = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &cfg_glm,
        RequestIntent::Free,
        None,
    );
    assert_eq!(
        Glm.encode(&ing_glm).body,
        Glm.encode(&ing_glm).body,
        "GLM 不确定性"
    );
}

/// 三家横向 3：块粒度不同（PROVIDERS.md 速查表 DeepSeek 128 / Kimi 256 / GLM
/// 64）。严格延长时（tools/system 原样不变，history 只在末尾追加），
/// `predicted_cache` 该按上一轮真实 `prompt_tokens` 向下取整到块边界
/// （`agent_core::PrefixImage::prompt_tokens` 的文档原话：「字节数换算不出
/// token 数，实测值才可靠」）。prev.prompt_tokens = 1000 时三家分别是
/// 896 / 768 / 960。
#[test]
fn predicted_cache_floors_to_each_providers_block_size_on_strict_extension() {
    assert_floors_to_block(&DeepSeek, "deepseek-v4-pro", 896);
    assert_floors_to_block(&Kimi, "kimi-k3", 768);
    assert_floors_to_block(&Glm, "glm-5.2", 960);
}

fn assert_floors_to_block(provider: &dyn Provider, model: &str, expected: u32) {
    let sys = [sys_chunk("base", "you are a helpful agent")];
    let tools = [tool_spec(
        "srv:fs/read",
        "read a file",
        serde_json::json!({"type": "object"}),
    )];
    let late_tools: [ToolSpec; 0] = [];
    let base_messages = [user_text(1, "hello")];
    let extended_messages = [user_text(1, "hello"), assistant_text(2, "hi there")];
    let cfg = config_for(model);

    // 第一轮：拿这家真实的前缀镜像形状（segments 的哈希是 adapter 自己的内部
    // 约定，测试不猜格式），手填 `prompt_tokens`——host 侧就是这么干的：encode
    // 时还不知道真实用量，usage 回来后才回填进 `PrefixImage`。
    let ing_v1 = ingredients(
        &sys,
        &base_messages,
        &tools,
        &late_tools,
        &cfg,
        RequestIntent::Free,
        None,
    );
    let mut prefix_v1 = provider.encode(&ing_v1).prefix;
    prefix_v1.prompt_tokens = Some(1000);

    // 第二轮：tools/system 原样不变，只在 history 末尾追加一条消息——这就是
    // 「严格延长」（PROVIDERS.md §一：DeepSeek/Kimi 只认扩展，GLM 是真前缀
    // 匹配，严格延长是它的子集，三家都该判 Ok，见 §「端到端验证」：三家的
    // 严格延长都判 Ok，第 2 层在正常多轮里零误报）。
    let ing_v2 = ingredients(
        &sys,
        &extended_messages,
        &tools,
        &late_tools,
        &cfg,
        RequestIntent::Free,
        Some(&prefix_v1),
    );
    let out_v2 = provider.encode(&ing_v2);

    assert_eq!(
        out_v2.drift, None,
        "{model}: tools/system 不变、history 只追加，不该判成漂移：{:?}",
        out_v2.drift
    );
    assert_eq!(
        out_v2.predicted_cache, expected,
        "{model}: prev.prompt_tokens=1000 严格延长时应按块粒度向下取整到 {expected}，实际 {}",
        out_v2.predicted_cache
    );
}
