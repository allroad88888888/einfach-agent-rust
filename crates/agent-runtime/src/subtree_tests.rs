//! 052 的单元面：detached 名单 / stash / 轮末孤儿判据，以及**红线 6 在 stash
//! 这一侧的门**。
//!
//! 这里不装 `RunnerCtx`（那要 provider + client + executor + 后端，端到端用例
//! 里已经装过），只测不需要它的那三个函数：`harvest_detached` / `take_orphans`
//! / `take_stash`。「detached 子不回写父」这条端到端的证据（父的消息里没多出
//! 一条）在 `tests/spawn_bg_indep_two_children_no_block.rs`。

use std::sync::Arc;

use agent_core::{
    AgentId, ChildConfig, ContentBlock, Epoch, Event, Session, StopReason, TokenUsage, ToolCallId,
    TurnStatus,
};

use super::*;

/// 一个 root 在 `Thinking`、底下挂一个已经答完的子 agent 的会话。
///
/// 返回的 `Epoch` 是**spawn 那一刻**的世代——`detach` 记的就是它（红线 6）。
fn session_with_a_finished_child() -> (Session, AgentId, Epoch) {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    // root 先进 `Thinking`：后面那条 `Cancel` 才是一次合法转移（Idle 收 Cancel
    // 是协议违规，不写任何 primitive，也就不会 bump 世代——测试会假绿）。
    let _ = session.step(Event::UserInput { agent: root.clone(), text: Arc::from("拆一个给后台") });

    let spawned_at = session.epoch();
    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput { agent: child.clone(), text: Arc::from("BGTASK") });
    let _ = session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("后台子的答案"))],
        stop: StopReason::EndTurn,
        usage: TokenUsage { prompt: 10, completion: 5, cached: None },
        prefix: agent_core::PrefixImage { segments: Vec::new(), prompt_tokens: None },
        adjustments: Vec::new(),
    });
    assert_eq!(session.status_of(&child), TurnStatus::Done { truncated: false });

    (session, child, spawned_at)
}

/// 后台子落终态 → 结果进 stash（等 053 的 collect 来领），detached 名单里划掉。
#[test]
fn a_finished_background_child_lands_in_the_stash() {
    let (session, child, spawned_at) = session_with_a_finished_child();
    let mut subtree = Subtree::default();
    subtree.detach(child.clone(), AgentId::root(), spawned_at);

    subtree.harvest_detached(&session);

    let stash = subtree.take_stash();
    assert_eq!(stash.len(), 1, "落终态的后台子该进 stash");
    assert_eq!(stash[0].child, child);
    assert_eq!(&*stash[0].content, "后台子的答案");
    assert!(!stash[0].is_error);
    assert!(subtree.take_orphans(&session).is_empty(), "进了 stash 就该从 detached 名单里划掉");
}

/// **红线 6，stash 这一侧的门。**
///
/// 后台子在飞期间世代被推走（这里用一次真的 `Cancel` 推，跟 undo 同一个机制），
/// 它那份结果就属于一个已经被回滚掉的世界——**不进 stash**。
///
/// 断言的强度靠下面那半条（`and_lands_in_it_without_the_bump`）撑着：同一份
/// fixture、只差一次 bump，一个空一个满。把 `harvest_detached` 里那行 epoch
/// 比对删掉，这条立刻红。
#[test]
fn a_stale_epoch_keeps_the_background_result_out_of_the_stash() {
    let (mut session, child, spawned_at) = session_with_a_finished_child();
    let mut subtree = Subtree::default();
    subtree.detach(child.clone(), AgentId::root(), spawned_at);

    // 在飞期间的取消/undo：世代 +1。子自己那份答案还原样躺在它的历史里
    // （`Cancel` 只推 root），但它属于上一代。
    let _ = session.step(Event::Cancel { agent: AgentId::root() });
    assert_ne!(session.epoch(), spawned_at, "取消该推走世代，否则这条测试是空跑的");

    subtree.harvest_detached(&session);

    assert!(subtree.take_stash().is_empty(), "过期世代的后台子结果不该进 stash（红线 6）");
}

/// 上一条的孪生：**不** bump 世代，同一份 fixture 该进 stash。
#[test]
fn and_lands_in_it_without_the_bump() {
    let (session, child, spawned_at) = session_with_a_finished_child();
    let mut subtree = Subtree::default();
    subtree.detach(child, AgentId::root(), spawned_at);

    subtree.harvest_detached(&session);

    assert_eq!(subtree.take_stash().len(), 1);
}

/// 还在跑的后台子既不进 stash 也还留在名单上——轮末清算要认得出它。
#[test]
fn a_running_background_child_is_the_orphan_candidate() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput { agent: child.clone(), text: Arc::from("BGTASK") });

    let mut subtree = Subtree::default();
    subtree.detach(child.clone(), root, session.epoch());
    subtree.harvest_detached(&session);

    assert!(subtree.take_stash().is_empty(), "还没落终态，没有结果可 stash");
    let orphans = subtree.take_orphans(&session);
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].child, child);
}

/// 绑了槽的（053 的 `collect` 就这么绑）**不是孤儿**：父正等着它，收割走的是
/// 槽位那条回写路。052 写死这条判据时它还恒真，053 接上之后它真的会挡人——
/// 「全 collect 完不触发孤儿收尾」这条验收就落在这一行上。
#[test]
fn a_child_bound_to_a_slot_is_not_an_orphan() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput { agent: child.clone(), text: Arc::from("BGTASK") });

    let mut subtree = Subtree::default();
    subtree.detach(child.clone(), root.clone(), session.epoch());
    let epoch = session.epoch();
    subtree.record(child, root, ToolCallId::new("call_collect"), epoch, crate::COLLECT_TOOL);

    assert!(subtree.take_orphans(&session).is_empty(), "有人在等它，就不该被当孤儿拆掉");
}

/// 已经不活的（spawn 那一轮被 undo 撤了）不算孤儿：没东西要拆，也没什么可告警。
#[test]
fn a_child_that_is_no_longer_live_is_not_an_orphan() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let spawned_at = session.epoch();

    let mut subtree = Subtree::default();
    subtree.detach(child.clone(), root, spawned_at);
    let _ = session.despawn_child(&child).unwrap();

    assert!(subtree.take_orphans(&session).is_empty());
}
