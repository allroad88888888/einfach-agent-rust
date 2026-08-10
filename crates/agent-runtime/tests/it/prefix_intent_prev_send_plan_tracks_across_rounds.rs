//! 103 额外验收：`PrevSendPlan` 真的在每次 `CallProvider` 之后被更新，不是
//! 停在 pristine 不动。
//!
//! 为什么这条不能只看压缩轮本身：压缩轮的判读（`prefix_intent_tier4_
//! boundary_advance.rs`）单独看，一个「`PrevSendPlan` 永远不更新」的实现照样
//! 能算对——那一轮本来就是 `send_plan_of != prev_send_plan_of`（新计划 vs.
//! 卡在 pristine 的旧值），不管 `prev_send_plan_of` 是「正确地停在换前之前那份
//! 计划」还是「压根没被写过、永远是 pristine」，两种情况在压缩轮当场都读出
//! `Intentional`，测试照样绿。
//!
//! 真正能戳穿这个 bug 的，是压缩轮**之后**的下一轮：只有 `PrevSendPlan` 真的
//! 跟着更新到「压缩轮实际发出去的那份计划」，`send_plan_of` 才会在下一轮跟它
//! 重新相等，`prev_send_plan_of` 才不会永远停在旧值上——不然压缩一次之后
//! **每一轮**都会被误判 `Intentional`，把第 1 层永久关掉。
//!
//! 这里直接断言 `Session` 的两个公开状态口子本身（103「定死的接口」），不经过
//! `DriftVerdict`：状态层的证据比「这一轮有没有告警」更直接——没有漂移的话，
//! `DriftVerdict` 全是 `Clean`，从外面根本看不出 `PrefixIntent` 算的是
//! `Reuse` 还是 `Intentional`。真正端到端逼出错判的场景见
//! `prefix_intent_reverse_lock.rs`。

use agent_core::{AgentId, Session};
use agent_runtime::run_turn;

use crate::support;

#[test]
fn prev_send_plan_follows_send_plan_after_the_round_that_set_it() {
    let dir = support::temp_dir("prefix-intent-prev-send-plan-tracks");

    let port = support::spawn_scripted_server(vec![
        support::sse_text("第一轮回复"),
        support::sse_text("压缩轮回复"),
        support::sse_text("压缩之后、没有再压缩的一轮"),
    ]);
    let (mut ctx, _events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    // 轮 1：从没发过请求。两者都该是 pristine——这条本身不能证明「真的在跟踪」，
    // 只是起点。
    run_turn(&mut session, &mut ctx, "第一句话").expect("第一轮不该是 source failure");
    assert_eq!(
        session.send_plan_of(&root),
        session.prev_send_plan_of(&root),
        "第一轮之后两者该继续相等（都还是 pristine）"
    );

    // 轮 2：压缩开火（第 4 档，边界推进）。`send_plan_of` 立刻变、
    // `prev_send_plan_of` 这一刻还没变——这正是这一轮该判 `Intentional` 的原因。
    let history_len = session.messages_of(&root).len();
    session.begin_turn();
    session
        .advance_boundary(&root, history_len, None)
        .expect("边界从 0 推到 history_len 该被接受");
    assert_ne!(
        session.send_plan_of(&root),
        session.prev_send_plan_of(&root),
        "replace_send_plan 立刻生效，但 PrevSendPlan 要等这一轮真的发出去才更新"
    );
    let compacted_plan = session.send_plan_of(&root);

    run_turn(&mut session, &mut ctx, "继续").expect("压缩轮不该是 source failure");

    // 压缩轮的 CallProvider 完成之后：`PrevSendPlan` 该跟上，变成「刚刚实际
    // 发出去的那份计划」——不是继续停在轮 1 的 pristine 值上。
    assert_eq!(
        session.prev_send_plan_of(&root),
        compacted_plan,
        "压缩轮完成后 PrevSendPlan 该被写成这一轮实际发出去的 SendPlan"
    );
    assert_eq!(
        session.send_plan_of(&root),
        session.prev_send_plan_of(&root),
        "压缩轮完成后两者该重新相等——这一刻起下一轮该读出 Reuse"
    );

    // 轮 3：没有再压缩。两者继续相等，证明「相等」不是压缩轮那一刻的巧合，
    // 而是 PrevSendPlan 真的持续跟着 send_plan_of 走。
    session.begin_turn();
    run_turn(&mut session, &mut ctx, "再继续").expect("第三轮不该是 source failure");
    assert_eq!(
        session.send_plan_of(&root),
        session.prev_send_plan_of(&root),
        "紧接着的下一轮没有新的压缩，两者该继续相等（intent 该读出 Reuse）"
    );
    assert_eq!(
        session.send_plan_of(&root),
        compacted_plan,
        "而且这个共同值还是压缩轮那份计划，不是被悄悄换回了别的东西"
    );
}
