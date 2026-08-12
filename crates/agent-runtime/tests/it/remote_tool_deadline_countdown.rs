//! 123 的 native 那一半：`remote_tool_deadline_in` 这个倒计时**跟
//! `sweep_remote_tool_deadlines` 判过期用的是同一条线**。
//!
//! # 为什么这条值得单独写
//!
//! 浏览器宿主（`agent-wasm`）执行一条 `web:` 工具是就地 `await` 页面的一个
//! Promise，没有「回去等命令」那一步可等，所以它把那次 await 做成了可打断的等待：
//! 倒计时归零 → 放弃这次执行 → 调 `sweep_remote_tool_deadlines_async` 收尾。
//! 于是**两边必须对同一条线说同一句话**：
//!
//! - 倒计时先归零、扫描却说「没有槽过期」→ 放弃了执行、槽还留在表里，下一圈把同一条
//!   工具再执行一遍（副作用做两遍），或者原地空转；
//! - 倒计时迟迟不归零、扫描却早就该判失败 → 挂住的回调拖着整个宿主（`send()` 在整轮
//!   期间握着 `live.borrow_mut()`）。
//!
//! 这两种病都**不报错**，所以判据焊死在这里：`Some(Duration::ZERO)` ⇔ 这一刻扫描
//! 真的把它划掉。
//!
//! # 反向锁在第一条测试里
//!
//! 「没超时的工具不得被误判成超时」跟「超时的工具必须被判失败」是两条独立的断言，
//! 后者已经由 `remote_tool_deadline_fails_the_call.rs`（060 验收第二、四条：到点划掉槽、
//! 晚到回传被安全拒绝）钉住，这里不重复造。第一条测试补的是它的反面：
//! **600ms 的活在 2s 的截止线下必须正常收尾**——一条工具结果、`is_error` 为假、
//! 一条超时事件都不该有。
//!
//! 时间常量按比例定（0.3 倍预算），不追求跟真机同量级——真机默认是 60s
//! （`agent-wasm` 的 `HOST_TOOL_TIMEOUT`），测试不真等。

use std::time::Duration;

use agent_core::{AgentId, ContentBlock, Session, ToolCallId, TurnStatus};
use agent_runtime::{
    RemoteToolOutput, RunnerEvent, ToolTable, remote_tool_deadline_in, resolve_remote_tool,
    run_turn, sweep_remote_tool_deadlines,
};

use crate::support::{build_ctx_with, spawn_scripted_server, sse_text, sse_tool_call, temp_dir};

/// 够跑完 `WORK` 还剩一大截：这条测试要断言的是「没到点」，机器忙一下不该让它变红。
const BUDGET: Duration = Duration::from_millis(2000);
/// 一条「干了正经活」的工具花掉的时间，占预算 0.3 倍——issue 里那条
/// 「3 秒的工具在 10 秒截止线下」的同一个比例。
const WORK: Duration = Duration::from_millis(600);

#[test]
fn a_tool_that_answers_inside_its_budget_is_never_judged_late() {
    let dir = temp_dir("remote-deadline-countdown");
    // 两跳：hop1 派一条远端工具，hop2 是拿到**正常结果**之后的收敛发言。
    let port = spawn_scripted_server(vec![
        sse_tool_call(
            "call_card",
            "browser_action",
            r#"{\"action\": \"render_card\"}"#,
        ),
        sse_text("卡片渲染好了。"),
    ]);
    let (ctx, events) = build_ctx_with(port, &dir, ToolTable::standard());
    let mut ctx = ctx.with_remote_tool_timeout(BUDGET);
    let mut session = Session::new(AgentId::root());

    let parked = run_turn(&mut session, &mut ctx, "渲染一张卡片")
        .expect("remote dispatch should not be a source failure");
    assert_eq!(parked, TurnStatus::ToolsPending);
    assert_eq!(ctx.pending_remote_tool_count(), 1);

    let root = AgentId::root();
    let call = ToolCallId::new("call_card");

    // 倒计时问的是**这一条槽**，不是全表最早的那条：查无此槽就是 `None`，
    // 不是「零」也不是预算本身——分不清这两者的话，宿主会把一条根本不存在的槽
    // 判成到点。
    assert_eq!(
        remote_tool_deadline_in(&ctx, &root, &ToolCallId::new("call_nobody")),
        None,
        "不存在的调用没有截止线可言"
    );
    let remaining = remote_tool_deadline_in(&ctx, &root, &call).expect("等待槽必须带截止线");
    assert!(
        !remaining.is_zero() && remaining <= BUDGET,
        "刚登记的槽该剩下「不超过预算、且还没归零」的时间，实际 {remaining:?}"
    );

    // 工具真的干了一会儿活。
    std::thread::sleep(WORK);

    let remaining = remote_tool_deadline_in(&ctx, &root, &call).expect("干活期间槽还在");
    assert!(
        !remaining.is_zero(),
        "{WORK:?} 的活在 {BUDGET:?} 的截止线下不该已经归零"
    );

    // 反向锁的正身：这一刻扫一次，**一个槽都不许被划掉**。
    let swept = sweep_remote_tool_deadlines(&mut session, &mut ctx)
        .expect("deadline sweep should not be a source failure");
    assert!(
        swept.is_none(),
        "没到点的槽被扫描判了失败：{swept:?}——这正是「正常工具被误判成超时」那条病"
    );
    assert_eq!(ctx.pending_remote_tool_count(), 1, "槽该原样还在等回传");

    // 正常回传：轮次收尾，模型拿到的是成功结果。
    let status = resolve_remote_tool(
        &mut session,
        &mut ctx,
        root.clone(),
        call.clone(),
        RemoteToolOutput::Success("{\"cardId\":\"card-1\"}".to_string()),
    )
    .expect("按时回传必须被接受");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert_eq!(
        remote_tool_deadline_in(&ctx, &root, &call),
        None,
        "收敛之后这条槽不该再有倒计时"
    );

    let results: Vec<(String, bool)> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1, "该正好有一条 tool_result: {results:#?}");
    assert!(!results[0].1, "按时返回的工具不许落 is_error：{results:#?}");
    assert!(
        results[0].0.contains("card-1"),
        "模型该拿到工具真正返回的正文：{results:#?}"
    );

    let events = events.borrow();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunnerEvent::ToolExecuted { is_error: true, .. })),
        "没超时就不该有任何 is_error 的执行事件：{events:#?}"
    );
}

/// 归零那一刻**扫描必须真的扫得到**。这条是浏览器宿主那个循环不空转的全部依据。
#[test]
fn the_countdown_hits_zero_exactly_when_the_sweep_expires_that_slot() {
    const SHORT: Duration = Duration::from_millis(80);

    let dir = temp_dir("remote-deadline-zero");
    let port = spawn_scripted_server(vec![
        sse_tool_call(
            "call_card",
            "browser_action",
            r#"{\"action\": \"render_card\"}"#,
        ),
        sse_text("宿主没响应，我改用文字说明。"),
    ]);
    let (ctx, _events) = build_ctx_with(port, &dir, ToolTable::standard());
    let mut ctx = ctx.with_remote_tool_timeout(SHORT);
    let mut session = Session::new(AgentId::root());

    let parked = run_turn(&mut session, &mut ctx, "渲染一张卡片")
        .expect("remote dispatch should not be a source failure");
    assert_eq!(parked, TurnStatus::ToolsPending);

    let root = AgentId::root();
    let call = ToolCallId::new("call_card");
    std::thread::sleep(SHORT + Duration::from_millis(40));

    assert_eq!(
        remote_tool_deadline_in(&ctx, &root, &call),
        Some(Duration::ZERO),
        "过了点之后倒计时该是零，不是一个负数也不是 `None`"
    );

    // 同一刻扫描：必须真的划掉它。倒计时归零却扫不出东西，宿主就会放弃执行、
    // 槽却还留在表里——下一圈把同一条工具再执行一遍。
    let status = sweep_remote_tool_deadlines(&mut session, &mut ctx)
        .expect("deadline sweep should not be a source failure")
        .expect("倒计时归零的这一刻扫描必须扫得到这条槽");
    assert!(
        status.is_terminal(),
        "超时之后这一轮该有结论，不是永久 ToolsPending：{status:?}"
    );
    assert_eq!(ctx.pending_remote_tool_count(), 0, "过期槽取走即消费");
    assert_eq!(
        remote_tool_deadline_in(&ctx, &root, &call),
        None,
        "划掉之后不该再有倒计时"
    );
}
