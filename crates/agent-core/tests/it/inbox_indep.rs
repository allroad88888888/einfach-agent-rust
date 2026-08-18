//! 205 独立测试（一）：`Slot::Inbox` 两档送达时机的**送达行为**——
//! `drain_now` / `drain_next_turn` 各只认自己那一档、顺序即投递顺序、
//! `NextTurn` 熬过 turn 边界、排空不动轮次账、空排空不落 entry。
//!
//! 黑盒来源：docs/issues/204-agent-mesh-decision.md §一/§二、
//! docs/issues/205-core-peek-and-inbox.md（「做什么」3 与「验收」两节）、
//! docs/INVARIANTS.md 红线 2 / 3 / 10 / 11、`lib.rs` 导出的类型签名。
//!
//! **实现体一行没读**：`command/inbox.rs`、`value/inbox.rs`、`graph/visibility.rs`、
//! `command/cross_read.rs` 四个文件全程没打开（WORKFLOW §三：看了实现，测的就只剩
//! 实现想到的那几条路径）。
//!
//! 另外两份复用这里的 `tree()` / `last_message_text()`：
//! `inbox_indep_deny.rs`（`deliver` 的六种显式拒绝）、
//! `inbox_indep_undo_restore.rs`（undo 不产生屏障 / 落盘往返带时机标记 /
//! 跨 agent 读）。

use std::sync::Arc;

use crate::support::session::new_session;
use crate::support::{
    provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event,
};
use agent_core::{AgentId, AtomKey, ChildConfig, ContentBlock, Deliver, Role, Session, Slot};

/// root + 两个直接子 agent：够覆盖「父→子」「子→父」「兄弟↔兄弟」三个方向。
pub(crate) fn tree() -> (Session, AgentId, AgentId, AgentId) {
    let mut session = new_session();
    let root = session.agent().clone();
    let a1 = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn a1");
    let a2 = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn a2");
    (session, root, a1, a2)
}

/// 排空之后落在对方 `Messages` 里的那条**长什么样**：`Role::User`、单个
/// `ContentBlock::Text`。形状在这里断言一次，正文交给各用例。
pub(crate) fn last_message_text(session: &Session, agent: &AgentId) -> String {
    let messages = session.messages_of(agent);
    let last = messages.last().expect("Messages 尾部该有一条").clone();
    assert_eq!(last.role, Role::User, "投递进来的是一条 user 消息");
    assert_eq!(last.blocks.len(), 1, "单个 ContentBlock");
    match &last.blocks[0] {
        ContentBlock::Text(text) => text.to_string(),
        other => panic!("该是 Text 块，得到 {other:?}"),
    }
}

fn drained_texts(session: &Session, agent: &AgentId) -> Vec<String> {
    session
        .messages_of(agent)
        .iter()
        .map(|m| match &m.blocks[0] {
            ContentBlock::Text(text) => text.to_string(),
            other => panic!("该是 Text 块，得到 {other:?}"),
        })
        .collect()
}

/// 投递只进收件箱（**不能直接往对方 `Messages` 上追加**，204 §二），排空才进
/// 对话，进完收件箱空。
#[test]
fn a_now_item_lands_at_the_tail_of_messages_and_leaves_the_inbox_empty() {
    let (mut session, root, a1, _a2) = tree();
    let before = session.messages_of(&a1).len();

    session
        .deliver(&root, &a1, Arc::from("把范围缩到 src/"), Deliver::Now)
        .expect("root 给活着的子 agent 投递该成功");

    assert_eq!(session.inbox_of(&a1).len(), 1, "投递先落收件箱");
    assert_eq!(
        session.messages_of(&a1).len(),
        before,
        "投递本身不碰 Messages——对方可能正有一个 provider 请求在飞"
    );

    assert_eq!(session.drain_now(&a1), 1, "搬了一条");
    assert!(session.inbox_of(&a1).is_empty(), "排空之后收件箱是空的");
    assert_eq!(
        session.messages_of(&a1).len(),
        before + 1,
        "Messages 尾部多了那条"
    );

    let text = last_message_text(&session, &a1);
    assert!(text.contains(root.as_str()), "正文要认得出发信人：{text}");
    assert!(text.ends_with("把范围缩到 src/"), "原文要原样在里面：{text}");
}

/// 顺序 = 投递顺序（红线 11：这些正文会进 prompt，顺序不定就是每轮都重排）。
#[test]
fn two_now_items_are_drained_in_delivery_order() {
    let (mut session, root, a1, a2) = tree();
    session
        .deliver(&root, &a1, Arc::from("第一句"), Deliver::Now)
        .expect("父→子");
    session
        .deliver(&a2, &a1, Arc::from("第二句"), Deliver::Now)
        .expect("兄弟→兄弟");

    let queued = session.inbox_of(&a1);
    assert_eq!(queued.len(), 2);
    assert_eq!(&*queued[0].text, "第一句", "收件箱本身就按投递顺序");
    assert_eq!(&*queued[1].text, "第二句");
    assert_eq!(queued[0].from, root);
    assert_eq!(queued[1].from, a2);

    assert_eq!(session.drain_now(&a1), 2, "一次搬两条");
    let texts = drained_texts(&session, &a1);
    assert_eq!(texts.len(), 2);
    assert!(texts[0].ends_with("第一句"), "先投的先进：{texts:?}");
    assert!(texts[1].ends_with("第二句"), "后投的后进：{texts:?}");
    assert!(texts[0].contains(root.as_str()));
    assert!(texts[1].contains(a2.as_str()));
}

/// **两档互不相吃**（上半）：`drain_now` 只搬 `Now`，`NextTurn` 那条原地不动。
#[test]
fn drain_now_moves_only_the_now_item_and_leaves_the_next_turn_one_in_place() {
    let (mut session, root, a1, _a2) = tree();
    session
        .deliver(&a1, &root, Arc::from("本轮就要"), Deliver::Now)
        .expect("子→父 Now");
    session
        .deliver(&a1, &root, Arc::from("下一轮再说"), Deliver::NextTurn)
        .expect("子→root NextTurn");
    assert_eq!(session.inbox_of(&root).len(), 2, "一个槽两个标记");

    assert_eq!(session.drain_now(&root), 1, "只该搬走 Now 那一条");

    let left = session.inbox_of(&root);
    assert_eq!(left.len(), 1, "NextTurn 那条原地还在");
    assert_eq!(left[0].when, Deliver::NextTurn);
    assert_eq!(&*left[0].text, "下一轮再说");
    assert_eq!(session.messages_of(&root).len(), 1, "只进去了一条");
    assert!(last_message_text(&session, &root).ends_with("本轮就要"));
}

/// **两档互不相吃**（下半）：`drain_next_turn` 反过来也只认自己那一档。
#[test]
fn drain_next_turn_moves_only_the_next_turn_item_and_leaves_the_now_one_in_place() {
    let (mut session, root, a1, _a2) = tree();
    session
        .deliver(&a1, &root, Arc::from("本轮就要"), Deliver::Now)
        .expect("Now");
    session
        .deliver(&a1, &root, Arc::from("下一轮再说"), Deliver::NextTurn)
        .expect("NextTurn");

    assert_eq!(session.drain_next_turn(), 1, "只该搬走 NextTurn 那一条");

    let left = session.inbox_of(&root);
    assert_eq!(left.len(), 1, "Now 那条原地还在");
    assert_eq!(left[0].when, Deliver::Now);
    assert_eq!(&*left[0].text, "本轮就要");
    assert_eq!(session.messages_of(&root).len(), 1);
    assert!(last_message_text(&session, &root).ends_with("下一轮再说"));
}

/// `NextTurn` **熬过 turn 边界**：本轮怎么跑都不该被 `Now` 那一档的定点顺手收走，
/// 直到下一轮 `begin_turn` 之后 `drain_next_turn` 来收。
#[test]
fn a_next_turn_item_survives_the_turn_boundary_until_drain_next_turn_takes_it() {
    let (mut session, root, a1, _a2) = tree();
    session
        .deliver(&a1, &root, Arc::from("留张条"), Deliver::NextTurn)
        .expect("留言给 root");

    // 这一轮照常跑完：Now 档的定点被调过若干次，root 自己也说过话。
    session
        .deliver(&root, &a1, Arc::from("顺手一句"), Deliver::Now)
        .expect("父→子");
    assert_eq!(session.drain_now(&a1), 1);
    assert_eq!(session.drain_now(&root), 0, "root 的 Now 档里什么都没有");
    let _ = session.step(user_input_event("这一轮"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "答完了"));
    assert_eq!(
        session.inbox_of(&root).len(),
        1,
        "一整轮跑完，留言还在收件箱里"
    );

    session.begin_turn();
    assert_eq!(
        session.inbox_of(&root).len(),
        1,
        "begin_turn 自己不排空——收由定点来做"
    );

    assert_eq!(session.drain_next_turn(), 1);
    assert!(session.inbox_of(&root).is_empty());
    assert!(last_message_text(&session, &root).ends_with("留张条"));
}

/// **`drain_now` 不碰 `TurnsUsed`**（204 §二 点名的、这一波唯一会静默出错的地方）：
/// 写成重置，两个 agent 互相喊话就是真无界，而且不报错、只烧 token。
#[test]
fn drain_now_does_not_touch_turns_used() {
    let (mut session, root, a1, _a2) = tree();
    let _ = session.step(user_input_event("干活"));
    let _ = session.step(provider_done_tool_use(
        session.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let _ = session.step(tool_result_event(session.epoch(), "call_1", "内容"));
    assert_eq!(session.turns_used(), 2, "fixture 没能造出 turns_used = 2");

    session
        .deliver(&a1, &root, Arc::from("插一句"), Deliver::Now)
        .expect("子→父");
    assert_eq!(session.drain_now(&root), 1);

    assert_eq!(
        session.turns_used(),
        2,
        "排空收件箱不是新一轮：重置了就把 204 §二 的停机论证拆了"
    );
    let entry = session.last_entry().expect("排空该留一条 entry");
    let turns_key = AtomKey::Agent(root.clone(), Slot::TurnsUsed);
    assert!(
        entry.changes.iter().all(|c| c.key != turns_key),
        "排空那条 entry 里根本不该出现 TurnsUsed：{:?}",
        entry.changes
    );
}

/// 没有待收的排空**什么都不做、不落 entry**（`History` 拒绝空步——undo 栈里不该
/// 出现按一下没反应的幽灵步）。只有另一档的条目时同样算「没有待收」。
#[test]
fn a_drain_with_nothing_pending_leaves_no_entry() {
    let (mut session, root, a1, _a2) = tree();

    let history_before = session.history_len();
    assert_eq!(session.drain_now(&a1), 0);
    assert_eq!(session.drain_next_turn(), 0);
    assert_eq!(session.history_len(), history_before, "空排空不落 entry");

    session
        .deliver(&a1, &root, Arc::from("下一轮"), Deliver::NextTurn)
        .expect("NextTurn");
    let history_before = session.history_len();
    assert_eq!(session.drain_now(&root), 0, "只有 NextTurn 条目 = 没有待收");
    assert_eq!(session.history_len(), history_before);
    assert_eq!(session.inbox_of(&root).len(), 1, "而且一条都没被动过");
}

/// 红线 11：进 prompt 的正文**逐字节确定**——同样的两步跑两遍，注入的那条一模一样
/// （没有时间戳、没有序号、没有随机 id）。
#[test]
fn the_injected_text_is_byte_identical_across_two_identical_runs() {
    fn run() -> String {
        let (mut session, root, a1, _a2) = tree();
        session
            .deliver(&root, &a1, Arc::from("同一句话"), Deliver::Now)
            .expect("deliver");
        assert_eq!(session.drain_now(&a1), 1);
        last_message_text(&session, &a1)
    }

    let first = run();
    assert!(!first.is_empty());
    assert_eq!(first, run(), "进 prompt 的东西不许带时间戳/随机 id（红线 11）");
}
