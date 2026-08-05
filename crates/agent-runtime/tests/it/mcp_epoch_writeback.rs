//! 043 验收第三条（**红线 6，本 issue 唯一违反后不报错的那条**）：MCP 调用在飞期间
//! bump epoch（模拟 undo/cancel），响应回来时**被丢弃、状态不含它**——回写前的 epoch
//! 比对真的挡住了幽灵结果。
//!
//! # 怎么把静默失败变红
//!
//! 时序焊死、留足余量：
//!
//! 1. hop1 立刻回一条 `mcp:slow/echo` 的工具调用 → dispatch 第四路起飞，credential
//!    存下**起飞那一刻的 epoch=0**，背景线程去问假 server。
//! 2. 假 server 收到 `tools/call` 后 **sleep 1s** 才回（慢响应）。
//! 3. 一个后台线程在 **250ms** 时翻取消标志 → 泵替 root 发 `Cancel` → **epoch bump 到
//!    1**、这一轮落 `Failed(Cancelled)`。
//! 4. 1s 时 server 的成功结果回来：`mcp_call::finish` 照常发一条 `ToolExecuted`
//!    （**证明结果真的回来了**，不是没跑到），组出 `ToolResult{epoch:0}` 喂回泵；
//!    `Session::step` 入口的 epoch 闸 `0 != 1` → **丢弃、不写消息历史**。
//!
//! 断言把两件事钉在一起：`ToolExecuted` 在（幽灵结果确实回来了）+ 消息历史里没有那段
//! 内容（闸把它挡在了世界之外）。**把闸拆掉，这条会立刻红**：幽灵 `ToolResult` 会被写
//! 进已经回滚掉的世界。

mod support;

use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use agent_core::{AgentId, ContentBlock, Failure, Session, TurnStatus};
use agent_runtime::{RunnerEvent, run_turn};

use support::mcp;

#[test]
fn in_flight_mcp_result_is_dropped_by_the_epoch_gate_after_a_mid_flight_cancel() {
    let dir = support::temp_dir("mcp-epoch");
    // 慢响应：server 收到 tools/call 后睡 1s 才回一条**成功**结果。
    let script = mcp::call_script("1", r#"{"content":[{"type":"text","text":"Echo: ghost"}]}"#);
    // 只挂 hop1——取消之后 run_turn 直接落终态，不会有 hop2。
    let port =
        support::spawn_scripted_server(vec![mcp::hop_tool_use("mcp_3Aslow_2Fecho", "call_ghost")]);
    // readOnly=true：可逆性与本测试无关（测的是 epoch，不是屏障），选它免得多一条屏障噪音。
    let (mut ctx, events) = mcp::build_ctx(
        port,
        &dir,
        "slow",
        vec![mcp::tool_entry("slow", "echo", true)],
        &script,
    );

    // 在飞期间（250ms）bump epoch：翻取消标志，泵会替 root 发 Cancel → epoch bump。
    let cancel = ctx.cancel_flag();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        cancel.store(true, Ordering::Relaxed);
    });

    let mut session = Session::new(AgentId::root());
    let start = Instant::now();
    let status = run_turn(&mut session, &mut ctx, "调个慢 MCP 工具");
    let elapsed = start.elapsed();

    // 取消真的生效了（epoch 被 bump 过——闸才有可比的对象）。
    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert!(
        elapsed >= Duration::from_millis(250) && elapsed < Duration::from_secs(4),
        "该在取消之后、慢响应回来并被闸挡掉之后收尾，不该挂死：实际 {elapsed:?}"
    );

    let events = events.borrow();
    // 幽灵结果**确实回来了**：finish 发过一条 ToolExecuted。没有这条，测试就是空的。
    assert!(
        events.iter().any(|e| matches!(
            &e.event,
            RunnerEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == "mcp:slow/echo"
        )),
        "慢响应该真的回来（finish 发过 ToolExecuted）——否则没测到闸：{events:#?}"
    );

    // 而它**没有**被写进消息历史：epoch 闸在回写前挡住了幽灵结果（红线 6）。
    let wrote_ghost = session.messages().iter().any(|m| {
        m.blocks.iter().any(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.contains("ghost"),
            ContentBlock::Text(t) => t.contains("ghost"),
            _ => false,
        })
    });
    assert!(
        !wrote_ghost,
        "幽灵结果被写进了已回滚的世界——回写前的 epoch 比对没挡住（红线 6）：{:#?}",
        session.messages()
    );
}
