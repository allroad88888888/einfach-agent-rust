//! 验收 6：`Event::epoch()` 提取器穷举——带 epoch 的五种变体
//! （`ProviderDone`/`ProviderFailed`/`ToolResult`/`ToolFailed`/`Timeout`）
//! 返回 `Some`，`UserInput`/`Cancel` 返回 `None`。这是 `step` 入口的闸门
//! 唯一依赖的判定：闸装在一处，不是转移表里每格各判一次。

mod support;

use std::sync::Arc;

use agent_core::{Epoch, ErrorClass, Event, StopReason, TokenUsage};

#[test]
fn epoch_bearing_variants_return_some() {
    let agent = support::agent();
    let epoch = Epoch::START.next();

    let cases: Vec<Event> = vec![
        Event::ProviderDone {
            agent: agent.clone(),
            epoch,
            blocks: vec![],
            stop: StopReason::EndTurn,
            usage: TokenUsage {
                prompt: 1,
                completion: 1,
                cached: None,
            },
            prefix: support::prefix_image(),
            adjustments: vec![],
        },
        Event::ProviderFailed {
            agent: agent.clone(),
            epoch,
            class: ErrorClass::Unknown,
            message: Arc::from("x"),
        },
        Event::ToolResult {
            agent: agent.clone(),
            epoch,
            call_id: support::call_id(),
            content: Arc::from("x"),
        },
        Event::ToolFailed {
            agent: agent.clone(),
            epoch,
            call_id: support::call_id(),
            error: Arc::from("x"),
        },
        Event::Timeout {
            agent: agent.clone(),
            epoch,
            call_id: None,
        },
        Event::Timeout {
            agent: agent.clone(),
            epoch,
            call_id: Some(support::call_id()),
        },
    ];

    for event in cases {
        assert_eq!(event.epoch(), Some(epoch), "{event:?} 应提取出 Some(epoch)");
    }
}

#[test]
fn user_input_and_cancel_return_none() {
    let agent = support::agent();
    assert_eq!(
        Event::UserInput {
            agent: agent.clone(),
            text: Arc::from("hi")
        }
        .epoch(),
        None,
        "UserInput 不带 epoch"
    );
    assert_eq!(Event::Cancel { agent }.epoch(), None, "Cancel 不带 epoch");
}
