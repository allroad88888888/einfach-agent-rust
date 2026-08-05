//! 流式累积器：尾帧 usage 分离、`content: null`、重复 `role`、`[DONE]`/心跳行。
//! 帧的 JSON 形状取自 probes/results/wire-shape.json 与 probes/PROVIDERS.md §三的
//! 真实观测——累积器是三家共享的类型（ADAPTER.md「trait 长什么样」），所以本文件
//! 里既有 DeepSeek 自己录的帧（`content:null`），也有把 PROVIDERS.md §三记录的
//! 「usage 另起一帧」「重复 role」这两处真实差异套进 DeepSeek 的累积器——这正是
//! 它必须扛住的输入，不是编造。
//!
//! `agent_providers::deepseek::DeepSeek` 经 `Provider` trait 调用，符合 issue 025
//! 「用 DeepSeek 验证接缝」的要求；不读 `src/deepseek/` 下任何文件。

mod support;

use agent_core::StopReason;
use agent_providers::{Provider, StreamEvent};

/// PROVIDERS.md §三 第 1 条：usage 可能在 finish 帧之后另起一帧，且那帧
/// `choices` 为空。假定每帧都有 `choices[0]` 的解码器要么 panic 要么丢 usage。
/// usage 数值取自 probes/results/wire-shape.json 里 deepseek 的 `stream.tail`
/// 记录（`prompt_tokens":18` / `prompt_cache_hit_tokens":0` 等字段逐字抄）；
/// 「usage 独立起一帧、choices 为空」的结构取自同一文档记录的 Kimi 真实帧——
/// 累积器要能同时扛住这两种真实观测到的形状。
#[test]
fn tail_usage_in_separate_empty_choices_frame_is_captured() {
    let mut acc = support::provider().accumulator();

    let finish_frame = r#"data: {"id":"x","object":"chat.completion.chunk","created":1,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
    let usage_frame = r#"data: {"id":"x","object":"chat.completion.chunk","created":1,"model":"deepseek-v4-pro","choices":[],"usage":{"prompt_tokens":18,"completion_tokens":15,"total_tokens":33,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":13},"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":18}}"#;
    let done_frame = "data: [DONE]";

    let finish_events = acc.push_line(finish_frame);
    assert!(
        finish_events
            .iter()
            .any(|e| matches!(e, StreamEvent::Finished(StopReason::EndTurn))),
        "finish 帧要产出 Finished(EndTurn)，实际: {finish_events:?}"
    );

    // 关键断言：choices 为空的这一帧不能 panic，且必须能拿到 UsageReady。
    let usage_events = acc.push_line(usage_frame);
    let usage = usage_events.iter().find_map(|e| match e {
        StreamEvent::UsageReady(u) => Some(u.clone()),
        _ => None,
    });
    let usage = usage.expect("choices 为空的尾帧必须仍然产出 UsageReady 事件");
    assert_eq!(usage.prompt, 18);
    assert_eq!(usage.completion, 15);
    // DeepSeek 的 prompt_cache_hit_tokens 为 0 时必须是 Some(0) 不是 None——
    // None/Some(0) 语义不同（TokenUsage 文档、PROVIDERS.md 未命中时字段仍在）。
    assert_eq!(usage.cached, Some(0));

    let done_events = acc.push_line(done_frame);
    assert!(done_events.iter().any(|e| matches!(e, StreamEvent::Done)));
    assert!(acc.is_done());

    let (_, stop, final_usage) = acc.finish();
    assert_eq!(stop, StopReason::EndTurn);
    assert_eq!(final_usage.prompt, 18);
    assert_eq!(final_usage.completion, 15);
    assert_eq!(final_usage.cached, Some(0));
}

/// PROVIDERS.md §三 第 2 条：DeepSeek 显式用 `"content": null` 表示空，不能靠
/// 「字段存在」判断有没有内容——存在但是 null 时不该产出空的 TextDelta。
/// 帧逐字取自 probes/results/wire-shape.json 的 deepseek `stream.tool.head`。
#[test]
fn explicit_content_null_does_not_emit_empty_text_delta() {
    let mut acc = support::provider().accumulator();

    let frames = [
        r#"data: {"id":"11088886-0ae3-4244-843d-16e1f74672da","object":"chat.completion.chunk","created":1785483332,"model":"deepseek-v4-pro","system_fingerprint":"fp_9954b31ca7_prod0820_fp8_kvcache_20260402","choices":[{"index":0,"delta":{"role":"assistant","content":null,"reasoning_content":""},"logprobs":null,"finish_reason":null}]}"#,
        r#"data: {"id":"11088886-0ae3-4244-843d-16e1f74672da","object":"chat.completion.chunk","created":1785483332,"model":"deepseek-v4-pro","system_fingerprint":"fp_9954b31ca7_prod0820_fp8_kvcache_20260402","choices":[{"index":0,"delta":{"content":null,"reasoning_content":"用户"},"logprobs":null,"finish_reason":null}]}"#,
        r#"data: {"id":"701eb37c-c0f1-4550-9c9e-02138ca0717e","object":"chat.completion.chunk","created":1785483333,"model":"deepseek-v4-pro","system_fingerprint":"fp_9954b31ca7_prod0820_fp8_kvcache_20260402","choices":[{"index":0,"delta":{"content":"好","reasoning_content":null},"logprobs":null,"finish_reason":null}],"usage":null}"#,
    ];

    let mut saw_empty_text_delta = false;
    let mut saw_real_text_delta = false;
    for f in frames {
        for ev in acc.push_line(f) {
            match ev {
                StreamEvent::TextDelta(t) if t.is_empty() => saw_empty_text_delta = true,
                StreamEvent::TextDelta(t) if &*t == "好" => saw_real_text_delta = true,
                _ => {}
            }
        }
    }

    assert!(!saw_empty_text_delta, "content:null 不该产出空的 TextDelta");
    assert!(saw_real_text_delta, "真正的 content 增量必须还在");
}

/// 重复 `role: "assistant"`（每帧都带）不能污染累积出来的文本——真实取自
/// probes/results/wire-shape.json 的 glm `stream.tail`：GLM 每帧都在 delta 里
/// 重复 role。累积器是三家共享类型，DeepSeek 走的是同一份累积逻辑，必须同样
/// 扛住这种输入。
#[test]
fn repeated_role_field_does_not_pollute_text() {
    let mut acc = support::provider().accumulator();

    let frames = [
        r#"data: {"id":"g1","created":1785483395,"object":"chat.completion.chunk","model":"glm-5.2","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":" 好"}}]}"#,
        r#"data: {"id":"g1","created":1785483395,"object":"chat.completion.chunk","model":"glm-5.2","choices":[{"index":0,"delta":{"role":"assistant","content":"好"}}]}"#,
        r#"data: {"id":"g1","created":1785483395,"object":"chat.completion.chunk","model":"glm-5.2","choices":[{"index":0,"finish_reason":"stop","delta":{"role":"assistant","content":""}}],"usage":{"prompt_tokens":28,"completion_tokens":185,"total_tokens":213,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":182}}}"#,
        "data: [DONE]",
    ];

    for f in frames {
        acc.push_line(f);
    }

    let (blocks, _, _) = acc.finish();
    let text: String = blocks
        .into_iter()
        .filter_map(|b| match b {
            agent_core::ContentBlock::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "好", "role 字段绝不能混进累积出来的文本");
    assert!(!text.contains("assistant"));
}

/// 非 `data:` 行（心跳注释、空行）必须安静地返回空事件，不出错、不 panic。
/// 心跳行取自 probes/results/wire-shape.json deepseek 的 `stream.text.head`
/// （`": keep-alive"`）。
#[test]
fn non_data_lines_are_ignored_without_error() {
    let mut acc = support::provider().accumulator();

    for line in [": keep-alive", "", ": keep-alive"] {
        let events = acc.push_line(line);
        assert!(
            events.is_empty(),
            "非 data: 行必须返回空事件，实际: {events:?}"
        );
    }
    assert!(!acc.is_done());
}

/// `data: [DONE]` 必须产出 `Done` 事件，且 `is_done()` 之后为真。
#[test]
fn done_marker_sets_is_done() {
    let mut acc = support::provider().accumulator();
    assert!(!acc.is_done());

    let events = acc.push_line("data: [DONE]");
    assert!(events.contains(&StreamEvent::Done));
    assert!(acc.is_done());
}
