//! 214 独立验收 · 第 3 条：**预算已经用完的终态 agent 不该被唤醒**。
//!
//! `docs/issues/214-wake-a-terminal-agent.md` §验收原文：「把目标的 `max_turns`
//! 调到刚好用完 → 投一条 → 没有新的 provider 调用、条目留在收件箱里、状态还是
//! 原来那个终态（不是被改写成 `Done{truncated:true}`）」。
//!
//! 这是 214 §三 点名的**唯一一处会静默出错的分支**：如果唤醒时不检查预算就直接
//! 调 `try_call_provider`，会落 `Done{truncated:true}`——那会把「因为预算耗尽而
//! 没被叫醒」和「自己正常答完了」两件事在状态上抹平；如果检查了预算却仍然发出
//! `CallProvider`，两个互相唤醒的 agent 就会无界地烧 token（`runner.rs` 模块文档
//! 「新的上界靠每个 agent 自己的 `MaxTurns`」这句话成立的前提就是这一格挡得住）。
//!
//! 这里直接用 core 的公开命令（`deliver` / `step`）构造「预算刚好用完的终态 +
//! 收件箱里有一条还没处理的 `Now` 投递」这个状态，再单独喂一次 `Event::Wake`——
//! 不经过 `agent-runtime` 的 `send_tool`，因为只有 root 的 `max_turns` 能通过公开
//! API（`Session::set_max_turns`）调到一个小数——子 agent 永远是默认的 32
//! （`ChildConfig` 不带 `max_turns` 字段），真跑 32 跳来撞子 agent 的顶不现实。
//! 这样测的是 `on_wake` 自己的撞顶判据，跟谁来调用它（`send_tool` 还是这里）无关。
//!
//! 黑盒来源：`docs/issues/214-wake-a-terminal-agent.md` §验收、
//! `command/txn.rs` 里 `turns_exhausted()` / `record_turn_attempt()` 的 rustdoc
//! （非禁读文件，明确写着「214 单独要这个读口：唤醒撞顶时它要的是『什么都不做』，
//! 而不是 `record_turn_attempt` 撞顶时那条 `Done{truncated:true}`」）。**没有读**
//! `command/transitions/wake.rs` 与 `agent-runtime/src/send_tool.rs`。

use std::sync::Arc;

use agent_core::{ChildConfig, Deliver, Event, InboxItem, Session, TurnStatus};

use crate::support::{agent, provider_done_end_turn, provider_done_tool_use, tool_result_event};

#[test]
fn a_terminal_agent_with_an_exhausted_turn_budget_is_not_woken() {
    let root = agent();
    let mut session = Session::new(root.clone());
    session.set_max_turns(2);

    // 把 root 精确地跑到「刚好用完预算」的终态：一次工具收敛（消耗第 1 次
    // CallProvider）+ 一次直接结束的回复（消耗第 2 次，正好等于 max_turns）。
    let _ = session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("先干一件事"),
    });
    assert_eq!(session.turns_used(), 1);
    let _ = session.step(provider_done_tool_use(
        session.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let _ = session.step(tool_result_event(session.epoch(), "call_1", "内容"));
    assert_eq!(
        session.turns_used(),
        2,
        "工具收敛之后那次重新调用也该计数（否则下面『刚好用完』的前提不成立）"
    );
    let _ = session.step(provider_done_end_turn(session.epoch(), "答完了"));
    assert_eq!(
        session.status(),
        TurnStatus::Done { truncated: false },
        "前提：它是自己正常答完的终态，不是被截断的"
    );
    assert_eq!(session.turns_used(), 2, "前提：turns_used == max_turns，刚好用完");

    // 另一个活着的 agent 往它收件箱里投一条 `Now`——deliver 本身只追加，不唤醒
    // 任何人（`command/inbox.rs` 模块文档），所以这一步不会提前把状态弄脏。
    let sender = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn 一个发信人");
    session
        .deliver(&sender, &root, Arc::from("CAPPED-PING 该被挡住"), Deliver::Now)
        .expect("投递本身应该成功——挡的是唤醒，不是投递");

    let before_status = session.status();
    let before_history_len = session.history_len();
    let before_turns_used = session.turns_used();

    let effects = session.step(Event::Wake {
        agent: root.clone(),
        epoch: session.epoch(),
    });

    assert!(
        effects.is_empty(),
        "预算耗尽时唤醒该什么都不产出（不发 CallProvider，也不发任何 Notice）：{effects:?}"
    );
    assert_eq!(
        session.status(),
        before_status,
        "状态该还是原来那个终态，不是被改写成 Done{{truncated:true}}"
    );
    assert_eq!(
        session.turns_used(),
        before_turns_used,
        "不该再计一次 CallProvider"
    );
    assert_eq!(
        session.history_len(),
        before_history_len,
        "不写 primitive ⇒ 不留 entry"
    );
    assert_eq!(
        session.inbox_of(&root),
        vec![InboxItem {
            from: sender,
            text: Arc::from("CAPPED-PING 该被挡住"),
            when: Deliver::Now,
        }],
        "条目原样留在收件箱里，没被读掉也没被丢弃"
    );
}
