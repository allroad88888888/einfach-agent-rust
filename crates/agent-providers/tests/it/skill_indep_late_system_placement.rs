//! 039 独立测试(agent-providers 层):`Ingredients::late_system` 的注入
//! placement 分策——038 探针实测钉死的结论(`probes/PROVIDERS.md` §七):
//! Kimi/GLM 消息级追加(~100% 保前缀,零代价),DeepSeek 改顶层 system 段尾部
//! (插一条新 `role:system` 消息会 120x 归零,改现有段尾保 ~91%)。
//!
//! 独立测试 agent 规则(跟 `three_providers.rs`/`encode_determinism.rs` 同一条,
//! 文件顶部注释抄的是它们的规则原文):只依据 `docs/ADAPTER.md`、
//! `probes/PROVIDERS.md`、`docs/INVARIANTS.md` 红线 11/12、
//! `agent-providers`/`agent-core` 的公开签名写成,**不看**任何一家
//! `src/deepseek|kimi|glm/encode.rs` 的实现体。
//!
//! 假定的公开签名(`late_system` 是 039 新增字段,未见实现体,接口经
//! docs/issues/039-skills-loading.md 给的名字 + 既有 `late_tools`/`system`
//! 字段命名惯例推定):
//!
//! ```ignore
//! pub struct Ingredients<'a> {
//!     ...既有字段(system/messages/tools/late_tools/config/intent/prev_prefix)...
//!     pub late_system: &'a [SystemChunk],
//! }
//! ```
//!
//! 断言手法是**数子串**而不是解析完整 JSON 结构(跟 `late_tools.rs` 的
//! `body_text.contains(...)` 同一种"按子串断言"风格):三家骨架都是 OpenAI
//! 兼容、`serde_json` 紧凑序列化(既有测试已验证过各家两次 encode 逐字节相同,
//! 字段名/引号形状是稳定的),数 `"role":"system"` 出现几次就能分辨"新增一条
//! 消息"和"改了已有那条的内容"这两种 placement,不需要知道 JSON 树的精确形状。

mod support;

use std::sync::Arc;

use agent_core::{
    ContentBlock, Message, MessageId, PrefixImage, RequestIntent, Role, Segment, SessionConfig,
    SystemChunk, ToolSpec,
};
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;
use agent_providers::{Ingredients, Provider};

fn sys(label: &str, text: &str) -> SystemChunk {
    SystemChunk {
        label: Arc::from(label),
        text: Arc::from(text),
    }
}

fn msg(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::User,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

fn cfg(model: &str) -> SessionConfig {
    SessionConfig {
        model: Arc::from(model),
        temperature: Some(0.7),
        max_tokens: Some(2048),
        context_window: Some(128_000),
    }
}

fn build_ingredients<'a>(
    system: &'a [SystemChunk],
    messages: &'a [Message],
    late_system: &'a [SystemChunk],
    config: &'a SessionConfig,
    prev_prefix: Option<&'a PrefixImage>,
) -> Ingredients<'a> {
    let empty_tools: &'a [ToolSpec] = &[];
    Ingredients {
        system,
        messages,
        tools: empty_tools,
        late_tools: empty_tools,
        late_system,
        config,
        intent: RequestIntent::Free,
        prev_prefix,
    }
}

fn system_role_message_count(body: &[u8]) -> usize {
    String::from_utf8_lossy(body)
        .matches("\"role\":\"system\"")
        .count()
}

fn contains(body: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(body).contains(needle)
}

/// Kimi/GLM:`late_system` 落地成**消息级**追加——多出一条 `role:"system"`,
/// 不改动已有那条的内容,且不该被判成前缀漂移(038:~100% 保前缀,零代价)。
fn assert_message_level(provider: &dyn Provider, model: &str) {
    let base = [sys("base", "你是一个称职的助手。")];
    let messages = [msg(1, "帮我查一下天气")];
    let skill = [sys("skill:testskill", "特别指令:回答末尾加【testskill】。")];
    let config = cfg(model);

    let ing_v1 = build_ingredients(&base, &messages, &[], &config, None);
    let mut prefix_v1 = provider.encode(&ing_v1).prefix;
    prefix_v1.prompt_tokens = Some(1000);
    let count_v1 = system_role_message_count(&provider.encode(&ing_v1).body);

    let ing_v2 = build_ingredients(&base, &messages, &skill, &config, Some(&prefix_v1));
    let out_v2 = provider.encode(&ing_v2);

    assert!(
        contains(&out_v2.body, "testskill"),
        "{model}: late_system 的内容必须真的进了 body"
    );
    assert!(
        contains(&out_v2.body, "称职的助手"),
        "{model}: 原有 system 内容不该被顶掉"
    );
    assert_eq!(
        system_role_message_count(&out_v2.body),
        count_v1 + 1,
        "{model}: 消息级追加——system 角色消息数该 +1(不是并进已有那条)"
    );
    assert_eq!(
        out_v2.drift, None,
        "{model}: 038 结论是消息级追加零代价,不该判成前缀漂移: {:?}",
        out_v2.drift
    );
}

#[test]
fn kimi_places_late_system_as_a_new_message() {
    assert_message_level(&Kimi, "kimi-k3");
}

#[test]
fn glm_places_late_system_as_a_new_message() {
    assert_message_level(&Glm, "glm-5.2");
}

/// DeepSeek:`late_system` 落地成**改现有顶层 system 段的尾部**——`role:"system"`
/// 的条数不变(还是原来那一条,只是变长了),不是插新消息(038:插新消息 120x
/// 归零,改段尾保 91%)。该段因此如实报漂移(`Segment::System`)——改都改了,
/// 不谎报没变(那正是「兜底第 1 层」要抓的东西)。
#[test]
fn deepseek_places_late_system_by_extending_the_existing_system_segment_tail() {
    let base = [sys("base", "你是一个称职的助手。")];
    let messages = [msg(1, "帮我查一下天气")];
    let skill = [sys("skill:testskill", "特别指令:回答末尾加【testskill】。")];
    let config = cfg("deepseek-v4-pro");

    let ing_v1 = build_ingredients(&base, &messages, &[], &config, None);
    let mut prefix_v1 = DeepSeek.encode(&ing_v1).prefix;
    prefix_v1.prompt_tokens = Some(1000);
    let count_v1 = system_role_message_count(&DeepSeek.encode(&ing_v1).body);

    let ing_v2 = build_ingredients(&base, &messages, &skill, &config, Some(&prefix_v1));
    let out_v2 = DeepSeek.encode(&ing_v2);

    assert!(
        contains(&out_v2.body, "testskill"),
        "late_system 内容必须真的进了 body"
    );
    assert!(
        contains(&out_v2.body, "称职的助手"),
        "原有 system 内容该保留(只是尾部追加,不是替换)"
    );
    assert_eq!(
        system_role_message_count(&out_v2.body),
        count_v1,
        "DeepSeek: 不该新插一条 role:system 消息(那条路 038 测出来是 120x 归零),\
         必须是并进已有那一条的尾部,system 消息条数该跟没激活时一样"
    );
    assert_eq!(
        out_v2.drift,
        Some(Segment::System),
        "改了顶层 system 段的尾部,该如实报 System 段漂移,不是悄悄声称没变"
    );

    // 有 late_system 时的 Adjustment(若实现报了)必须认得出跟这次注入有关——
    // 不锁死具体变体名(issue 原文给的候选是 `LateSystemReshapedPrefix` 或复用
    // 现有变体,两者都可能),只要求可辨认;不报也不算错(issue 原文:「若实现报了」)。
    if !out_v2.adjustments.is_empty() {
        let debug = format!("{:?}", out_v2.adjustments).to_lowercase();
        assert!(
            debug.contains("system") || debug.contains("late") || debug.contains("prefix"),
            "报了 Adjustment 但内容认不出跟 late_system 注入有关: {debug}"
        );
    }
}

/// 红线 11:三家各自,同一份带 `late_system` 的料两次 encode 逐字节相同。
#[test]
fn each_provider_with_late_system_is_still_byte_deterministic() {
    let base = [sys("base", "你是一个称职的助手。")];
    let messages = [msg(1, "帮我查一下天气")];
    let skill = [sys("skill:testskill", "特别指令。")];

    let providers: [(&dyn Provider, &str); 3] = [
        (&DeepSeek, "deepseek-v4-pro"),
        (&Kimi, "kimi-k3"),
        (&Glm, "glm-5.2"),
    ];
    for (provider, model) in providers {
        let config = cfg(model);
        let built = build_ingredients(&base, &messages, &skill, &config, None);
        let a = provider.encode(&built).body;
        let b = provider.encode(&built).body;
        assert_eq!(
            a, b,
            "{model}: 带 late_system 的料两次 encode 必须逐字节相同"
        );
    }
}
