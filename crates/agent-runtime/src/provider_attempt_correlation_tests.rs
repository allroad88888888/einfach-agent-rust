use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use agent_core::{DriftVerdict, PrefixImage, SessionConfig, StopReason, TokenUsage};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::{Client, StreamOutcome};
// 114b：`ProviderCall.deadline` 的字段类型已经是 `web_time::Instant`（见
// `provider_call.rs`），这里显式跟着改来源。
use web_time::Instant;

use super::*;
use crate::event::RunnerEvent;
use crate::provider_attempt::ProviderAttemptId;
use crate::provider_message::{self, ProviderMessage};
use crate::tool_table::ToolTable;

fn config() -> SessionConfig {
    SessionConfig {
        model: Arc::from("test-model"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

fn build() -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "test-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        config(),
        crate::persist::open_backend(None, |_| {}),
        Box::new(move |event| observed.borrow_mut().push(event)),
    );
    (ctx, events)
}

fn call(ctx: &RunnerCtx, attempt: ProviderAttemptId) -> ProviderCall {
    let selection = ctx.execution_binding_for(None).unwrap();
    ProviderCall {
        agent: AgentId::root(),
        attempt,
        epoch: Epoch::START,
        deadline: Instant::now() + Duration::from_secs(1),
        binding: selection.binding,
        guard_scope: selection.guard_scope,
        drift: DriftVerdict::Clean,
        predicted_cache: 0,
        adjustments: Vec::new(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        one_shot: false,
        hold_deltas: false,
        cancel_token: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn stale_messages_cannot_touch_same_agent_retry_in_same_epoch() {
    let (mut ctx, events) = build();
    let stale = ProviderAttemptId::allocate();
    let current = ProviderAttemptId::allocate();
    let agent = AgentId::root();
    let mut calls = vec![call(&ctx, current)];
    let mut pending = VecDeque::new();

    let messages = [
        ProviderMessage::delta(
            agent.clone(),
            stale,
            RunnerEvent::TextDelta(Arc::from("late")),
        ),
        ProviderMessage::done(
            agent.clone(),
            stale,
            Ok(StreamOutcome::Cancelled),
            Vec::new(),
            StopReason::EndTurn,
            TokenUsage {
                prompt: 0,
                completion: 0,
                cached: None,
            },
        ),
        ProviderMessage::gone(agent, stale),
    ];

    for message in messages {
        assert!(provider_message::land(&mut ctx, &mut calls, &mut pending, message).is_none());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].attempt, current);
        assert!(pending.is_empty());
        assert!(events.borrow().is_empty());
    }
}

/// **117 验收第二条**：`sync_channel(0)` 换成 `futures` 的 `mpsc::channel(0)` 之
/// 后，「泵划掉凭据」与「发送端手上那条已经写进 channel 的增量」之间第一次出现
/// 了时间窗——旧的会合语义下这个窗口不存在（发送端会一直停在 `send` 里，泵不收
/// 它就写不进去）。这条测试把那个窗口摆出来，断言晚到的增量按
/// `(agent, attempt)` 找不到凭据而被丢弃。
///
/// 时序焊死，没有一处靠睡眠或运气：
///
/// 1. 起一个真的 IO 载体（[`crate::io_task::run`]，跟生产代码同一个函数，只是
///    行源换成手工喂的 channel），凭据进在飞表。
/// 2. 喂一行 → **只推 IO future、不收消息**：增量此刻真的躺在 channel 的槽位里。
/// 3. 泵在这一刻划掉凭据并**开一次同 agent 的重试**（超时那条路的原样：
///    `deadline::sweep` remove 掉旧凭据 → `Event::Timeout` → 转移表决定重试 →
///    `provider_call::start` 放进一张新凭据）。重试与被放弃的那次**共用一个
///    epoch**（重试不 bump 世代），所以此刻唯一还能分辨两者的东西就是
///    `attempt`——红线 6 那道 epoch 闸在这里帮不上忙。
/// 4. 收：消息**确实回来了**（`receive` 拿到了它，不是没跑到），而
///    `provider_message::land` 认不出凭据 → 原地丢弃，不产事件、不产待办。
///
/// 断言把两件事钉在一起，跟 `tests/it/mcp_epoch_writeback.rs` 同款：幽灵确实回
/// 来了 + 它没有留下任何痕迹。**把 `land` 里那句 `position(...)` 拆掉，这条会
/// 立刻红**。
#[test]
fn a_delta_already_in_the_channel_is_dropped_once_its_credential_is_gone() {
    use agent_providers::Provider;
    use futures_channel::mpsc;

    use crate::io_bus::IoBus;
    use crate::io_stream::StreamItem;
    use crate::io_task::{self, IoMsg};

    let (mut ctx, events) = build();
    let mut bus = IoBus::new(Duration::from_millis(20));
    let (mut lines, line_source) = mpsc::channel::<StreamItem>(4);

    let attempt = ProviderAttemptId::allocate();
    bus.start(io_task::run(
        bus.sender(),
        bus.sender(),
        AgentId::root(),
        attempt,
        DeepSeek.accumulator(),
        line_source,
    ));
    let mut calls = vec![call(&ctx, attempt)];
    let mut pending = VecDeque::new();

    // 1–2. 一行文本增量进 channel，泵还没收。
    lines
        .try_send(StreamItem::Line(
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"幽灵增量"},"finish_reason":null}]}"#
                .to_string(),
        ))
        .unwrap();
    bus.drive_tasks_once();

    // 3. 旧凭据被划掉，同 agent 的重试立刻补上一张新的（同一个 epoch）。
    let retry = ProviderAttemptId::allocate();
    calls.clear();
    calls.push(call(&ctx, retry));

    // 4. 幽灵确实回到了泵。
    let message = crate::block_on(bus.receive(Duration::from_secs(1)))
        .expect("那条增量已经写进 channel 了，泵必须收得到——收不到这条测试就是空的");
    let IoMsg::Provider(message) = message else {
        panic!("该是一条 provider 消息");
    };
    assert_eq!(message.kind(), "delta");
    assert_eq!(message.attempt(), attempt, "它属于那次已经被放弃的 attempt");

    provider_message::land(&mut ctx, &mut calls, &mut pending, message);

    assert!(
        pending.is_empty(),
        "幽灵增量不该变成任何待办事件——一旦变成事件，它就会被喂进 Session::step 写进消息历史"
    );
    assert!(
        events.borrow().is_empty(),
        "幽灵增量也不该经回调发给宿主（它会被当成重试那一次的输出流）：{} 条",
        events.borrow().len()
    );
    assert_eq!(calls.len(), 1, "重试那张凭据必须原封不动地还在飞");
    assert_eq!(calls[0].attempt, retry);
}
