//! 108 验收：`context_window: None` → 两档都不触发（096 第一问：不许 `unwrap`，
//! 也不许瞎猜）。
//!
//! 用 `crate::support::build_ctx`（不是本系列自己的 `ladder_support::build_ctx`）
//! ——那个共用脚手架本来就把 `SessionConfig.context_window` 焊死成 `None`，
//! 正好是这条测试要的默认值，不需要另开一个口子。usage 故意钦定成一个荒谬的
//! 大数（远超任何合理窗口），如果实现在哪里偷偷 `unwrap_or` 成了某个默认窗口，
//! 这条测试会当场炸。

use agent_core::{AgentId, Notice, Session, TurnStatus};
use agent_runtime::{RunnerEvent, run_turn};

use crate::support;
use crate::support::ScriptedResponse;

/// 荒谬的高 usage：如果 `context_window: None` 被悄悄换成任何有限默认值，
/// 这个数字在几乎所有窗口大小下都会冲过 85%。
const ABSURD_PROMPT_TOKENS: u32 = 999_999_999;

fn leak(lines: Vec<String>) -> Vec<&'static str> {
    lines
        .into_iter()
        .map(|l| -> &'static str { Box::leak(l.into_boxed_str()) })
        .collect()
}

fn text_with_usage(text: &str, prompt_tokens: u32) -> Vec<String> {
    let chunk1 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": 5,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": prompt_tokens
        }
    });
    vec![
        format!("data: {chunk1}"),
        format!("data: {chunk2}"),
        "data: [DONE]".to_string(),
    ]
}

#[test]
fn neither_tier_fires_when_context_window_is_none() {
    let dir = support::temp_dir("ladder-context-window-none");
    let script: Vec<_> = (0..8)
        .map(|_| ScriptedResponse::Sse(leak(text_with_usage("继续", ABSURD_PROMPT_TOKENS))))
        .collect();
    let (port, bodies) = support::spawn_recording_server(script);
    let (mut ctx, events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    for i in 0..8 {
        // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了
        // 不报错，新的 `UserInput` 撞上上一轮的 `Done` 会被判成
        // `Notice::ProtocolViolation`，这一轮根本没发生过任何请求，下面
        // `bodies.len()` 那条硬指标就测不出「真的什么都没触发」和「压根没跑」
        // 的区别。
        if i > 0 {
            session.begin_turn();
        }
        let text = format!("第 {i} 轮，usage 荒谬地高，但 context_window 是 None");
        let status = run_turn(&mut session, &mut ctx, &text)
            .unwrap_or_else(|e| panic!("第 {i} 轮不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "第 {i} 轮");
    }

    assert_eq!(bodies.lock().unwrap().len(), 8, "8 轮该恰好 8 次 provider 调用，没有额外的压缩子请求");

    let plan = session.send_plan_of(&root);
    assert!(plan.cleared().is_empty(), "context_window: None 时第 2 档不该清过任何东西");
    assert_eq!(plan.boundary(), 0, "context_window: None 时第 3 档不该动过边界");
    assert_eq!(plan.summary(), None);
    assert!(
        session.children_of(&root).is_empty(),
        "不该有任何压缩子被 spawn 过"
    );

    let events = events.borrow();
    let notices: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !notices.iter().any(|n| matches!(
            n,
            Notice::CompactionSummaryReceived | Notice::CompactionFailed
        )),
        "不该有任何压缩相关的 Notice：{notices:?}"
    );
}
