//! 流式累积器：工具 `arguments` 分片累加。
//!
//! probes 没有录到一份「工具参数按 index 分三片流式返回」的 DeepSeek 原始帧
//! （`probes/results/wire-shape.json` 里跟工具相关的记录都是非流式的完整
//! `tool_calls` 数组，见 `parallel.tool_calls`）。PROVIDERS.md §三明确写了
//! 「工具参数都按 index 累加」，且三家骨架都是 OpenAI 兼容流协议——本文件按
//! 这个已确认的通用 delta 形状（`delta.tool_calls[].function.arguments` 按
//! `index` 分片）构造三片输入，`id`/`name`/`arguments` 的取值直接抄
//! `parallel.tool_calls` 里 DeepSeek 那条真实记录（`call_00_...`、
//! `get_weather`、`{"city": "北京"}`），只是把已知的完整 JSON 切成三段模拟
//! 流式到达。

use crate::support;
use agent_providers::{Provider, StreamEvent};
use serde_json::json;

#[test]
fn tool_call_arguments_accumulate_across_three_chunks_and_start_fires_once() {
    let mut acc = support::provider().accumulator();

    // 第一片：index/id/name 齐了，arguments 只到一半。
    let chunk1 = r#"data: {"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_00_tsdA0CxopR9nvmG8nbrq3971","type":"function","function":{"name":"get_weather","arguments":"{\"city\""}}]},"finish_reason":null}]}"#;
    // 第二片：只有 index，继续拼 arguments。
    let chunk2 = r#"data: {"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":": \""}}]},"finish_reason":null}]}"#;
    // 第三片：收尾，凑成合法 JSON。
    let chunk3 = r#"data: {"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"北京\"}"}}]},"finish_reason":"tool_calls"}]}"#;

    let events1 = acc.push_line(chunk1);
    let started: Vec<_> = events1
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolCallStarted { .. }))
        .collect();
    assert_eq!(started.len(), 1, "id+name 齐了要发一次 ToolCallStarted");
    match &started[0] {
        StreamEvent::ToolCallStarted { index, id, name } => {
            assert_eq!(*index, 0);
            assert_eq!(id.0.as_ref(), "call_00_tsdA0CxopR9nvmG8nbrq3971");
            assert_eq!(name.as_ref(), "get_weather");
        }
        _ => unreachable!(),
    }

    // 后续两片只是参数片段，不该再发 ToolCallStarted。
    let events2 = acc.push_line(chunk2);
    assert!(
        !events2
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallStarted { .. })),
        "id+name 已经报过一次，第二片不该重复发 ToolCallStarted"
    );

    let events3 = acc.push_line(chunk3);
    assert!(
        !events3
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallStarted { .. }))
    );

    let (blocks, _, _) = acc.finish();
    let tool_use = blocks
        .into_iter()
        .find_map(|b| match b {
            agent_core::ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
            _ => None,
        })
        .expect("finish() 必须拼出完整的 ToolUse 块");

    assert_eq!(tool_use.0.0.as_ref(), "call_00_tsdA0CxopR9nvmG8nbrq3971");
    assert_eq!(tool_use.1.as_ref(), "get_weather");
    assert_eq!(
        *tool_use.2,
        json!({"city": "北京"}),
        "三片 arguments 拼接后必须是合法且完整的 JSON"
    );
}
