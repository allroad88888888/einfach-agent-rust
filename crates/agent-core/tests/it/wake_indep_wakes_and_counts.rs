//! 214 独立验收 · 第 2 + 5 条：**唤醒真的能重新发起调用、`TurnsUsed` 接着往上数、
//! 且不重复往历史里写话**。
//!
//! 两条钉在一个文件里，因为它们守的是**同一次成功唤醒**的两个不同侧面，拆开测
//! 反而要各自搭一遍完全一样的前置状态：
//!
//! - **第 2 条**（214 §三点名「这一波唯一会静默出错的地方」）：`turns_used = N`
//!   的终态 agent 被唤醒之后必须是 `N + 1`，不是被重置成 1。写成重置的话，
//!   `runner.rs` 模块文档那句「新的上界靠每个 agent 自己的 `MaxTurns`」就不成立
//!   ——两个互相唤醒的 agent 会真无界，而且**测试照样能全绿**，只在账单上暴露
//!   （DeepSeek 上 120 倍，这是本仓最贵的一类 bug 的通用形状）。
//! - **第 5 条**：唤醒转移自己不 `push_message`——那句话已经由 `Session::drain_now`
//!   （205/206 的排空定点）写进 `Messages` 了。这里手动模拟调用方在触发 `Wake`
//!   之前该做的事（`deliver` + `drain_now`），确认 `Event::Wake` 真的不重复写一遍
//!   ——`Event::Wake` 的形状本身也印证这件事：它没有携带正文的字段，结构上就
//!   写不出第二份。
//!
//! 黑盒来源：`docs/issues/214-wake-a-terminal-agent.md` §验收、
//! `command/inbox.rs` 中 `drain_now` 的 rustdoc（非禁读文件）、
//! `engine/event.rs` 里 `Event::Wake` 的字段与文档（非禁读文件：「不带正文：话已经
//! 由 `Session::drain_now` 进了 `Messages`」）。**没有读** `command/transitions/wake.rs`
//! 与 `agent-runtime/src/send_tool.rs`。

use std::sync::Arc;

use agent_core::{ChildConfig, ContentBlock, Deliver, Effect, Event, Session, TurnStatus};

use crate::support::{agent, provider_done_end_turn, provider_done_tool_use, tool_result_event};

/// 把 root 跑到 `turns_used == 2` 的终态，再从另一个活着的 agent 那儿投一条
/// `Now` 并当场排空——这正是 `send_tool` 在唤醒一个终态 agent 之前该做的两步
/// （deliver 落地、drain_now 把它搬进 `Messages`），只是这里手动做，好让唤醒
/// 转移本身被单独测到。
fn done_agent_with_a_drained_wakeup_message() -> (Session, agent_core::AgentId, &'static str) {
    let root = agent();
    let mut session = Session::new(root.clone());

    let _ = session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("第一件事"),
    });
    let _ = session.step(provider_done_tool_use(
        session.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let _ = session.step(tool_result_event(session.epoch(), "call_1", "内容"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "第一次答完了"));
    assert_eq!(session.status(), TurnStatus::Done { truncated: false });
    assert_eq!(session.turns_used(), 2, "前提：两次 CallProvider 之后落终态");

    let sender = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn 一个发信人");
    let needle = "WAKEUP-COUNT-唤醒该带着这句话";
    session
        .deliver(&sender, &root, Arc::from(needle), Deliver::Now)
        .expect("投递该成功");
    let moved = session.drain_now(&root);
    assert_eq!(moved, 1, "前提：drain_now 真的把它搬进了 Messages");

    (session, root, needle)
}

/// 一条消息里有没有 `needle`——跟 `send_indep_support::message_contains` 同一个
/// 判法，这里不复用那份 runtime 侧的夹具（agent-core 的独立测试不该依赖
/// agent-runtime 的测试夹具，两个 crate 的测试二进制本来就不共享）。
fn message_contains(message: &agent_core::Message, needle: &str) -> bool {
    message.blocks.iter().any(|block| match block {
        ContentBlock::Text(t) | ContentBlock::Thinking(t) => t.contains(needle),
        ContentBlock::ToolResult { content, .. } => content.contains(needle),
        ContentBlock::ToolUse { input, .. } => input.to_string().contains(needle),
    })
}

#[test]
fn waking_a_done_agent_calls_provider_again_and_advances_turns_used_without_resetting() {
    let (mut session, root, _needle) = done_agent_with_a_drained_wakeup_message();

    let effects = session.step(Event::Wake {
        agent: root.clone(),
        epoch: session.epoch(),
    });

    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { agent, .. } if *agent == root)),
        "唤醒该真的发一条 CallProvider：{effects:?}"
    );
    assert_eq!(
        session.status(),
        TurnStatus::Thinking,
        "发出 CallProvider 之后状态该回到 Thinking，等着这次回执"
    );
    assert_eq!(
        session.turns_used(),
        3,
        "唤醒之前是 2，唤醒该接着数到 3——写成重置的话这里会是 1"
    );
}

#[test]
fn the_drained_message_appears_in_messages_exactly_once_after_wake() {
    let (mut session, root, needle) = done_agent_with_a_drained_wakeup_message();

    let before_count = session
        .messages_of(&root)
        .iter()
        .filter(|m| message_contains(m, needle))
        .count();
    assert_eq!(before_count, 1, "前提：drain_now 已经把它写进历史一次");

    let _ = session.step(Event::Wake {
        agent: root.clone(),
        epoch: session.epoch(),
    });

    let after_count = session
        .messages_of(&root)
        .iter()
        .filter(|m| message_contains(m, needle))
        .count();
    assert_eq!(
        after_count, 1,
        "唤醒转移自己不该再 push 一遍——它没有正文字段，写两遍就是同一句话进两次历史"
    );
}
