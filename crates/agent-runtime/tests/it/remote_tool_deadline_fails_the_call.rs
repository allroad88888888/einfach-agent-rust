//! 060 验收第二、四条：客户端**永不回传**时，等待槽到点被判失败，轮次收尾；
//! 超时之后**迟到的回传**被安全拒绝。
//!
//! 正常路径靠宿主 `POST /tool_result`，异常路径靠用户 `Cancel`。可「前端崩了 /
//! 网关挂了 / 客户端根本没实现这个工具」这三种情况下两条都不会来——060 之前
//! `PendingRemoteTool` 没有任何截止线，`sweep_deadlines` 也只扫 provider 的在飞
//! 表，于是会话**永久**停在 `ToolsPending`。
//!
//! 超时不由泵驱动（泵在远端等待期间已经收工返回，控制权在宿主的命令队列上，
//! 见 `crate::deadline` 模块文档），所以这里模拟的正是宿主该做的两步：问
//! `next_remote_deadline` 等到点，调 `sweep_remote_tool_deadlines`。
//!
//! 超时压到 80ms（`with_remote_tool_timeout`）——真实默认是 10 分钟，测试不真等。

mod support;

use std::time::{Duration, Instant};

use agent_core::{AgentId, ContentBlock, Session, ToolCallId, TurnStatus};
use agent_runtime::{RemoteToolOutput, RunnerEvent, ToolTable, resolve_remote_tool, run_turn, sweep_remote_tool_deadlines};

use support::{build_ctx_with, spawn_scripted_server, sse_text, sse_tool_call, temp_dir};

const BUDGET: Duration = Duration::from_millis(80);

#[test]
fn a_remote_tool_nobody_ever_answers_is_failed_at_its_deadline_and_the_turn_ends() {
    let dir = temp_dir("remote-deadline");
    // 两跳：hop1 调一个**真在表里**的远端工具，hop2 是超时结果回到模型之后它的
    // 收敛发言。没有任何东西会回传 hop1 那次调用——这正是本测试模拟的世界。
    let port = spawn_scripted_server(vec![
        sse_tool_call("call_card", "browser_action", r#"{\"action\": \"render_card\"}"#),
        sse_text("宿主没响应，我改用文字说明。"),
    ]);
    let (ctx, events) = build_ctx_with(port, &dir, ToolTable::standard());
    let mut ctx = ctx.with_remote_tool_timeout(BUDGET);
    let mut session = Session::new(AgentId::root());

    // 第一步：轮次派出远端调用就地停住（既有行为，不变）。
    let parked = run_turn(&mut session, &mut ctx, "渲染一张卡片");
    assert_eq!(parked, TurnStatus::ToolsPending, "远端调用派出后本轮该在这里停住等回传");
    assert_eq!(ctx.pending_remote_tool_count(), 1);

    // 第二步：宿主空闲等命令——等到截止线，没等到任何回传。
    let deadline = ctx.next_remote_deadline().expect("060：等待槽必须带截止线");
    assert!(deadline <= Instant::now() + BUDGET, "截止线该是登记那一刻 + 预算，不是别的什么时刻");
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(20));

    // 第三步：到点扫一次 —— 槽被判失败，事件泵接着把这一轮跑完。
    let status = sweep_remote_tool_deadlines(&mut session, &mut ctx).expect("到点该有槽过期");
    assert_eq!(status, TurnStatus::Done { truncated: false }, "超时该让轮次收尾，不是永久 ToolsPending");
    assert_eq!(ctx.pending_remote_tool_count(), 0, "过期槽取走即消费");
    assert_eq!(ctx.next_remote_deadline(), None);

    // 模型看到的是一条 `is_error` 的 tool_result（不是 panic、不是静默丢弃）。
    let results: Vec<(String, bool)> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, is_error, .. } => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1, "该正好有一条 tool_result: {results:#?}");
    assert!(results[0].1, "超时该落 is_error 让模型自己收敛: {results:#?}");
    assert!(results[0].0.contains("remote_tool_timeout"), "错误里该说清是超时: {results:#?}");

    // 可见性跟真回传落地同款：宿主/UI 看到这次调用有了结局。
    let events = events.borrow();
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunnerEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == "browser_action"
        )),
        "超时也该发一条 ToolExecuted(is_error)：{events:#?}"
    );

    // 060 验收第四条：**迟到的回传**（客户端终于醒了）撞不进任何槽——`take_remote_tool`
    // 找不到 → 既有的 `RemoteToolResultError` 那条路，宿主翻成 `TransportTrouble`。
    let late = resolve_remote_tool(
        &mut session,
        &mut ctx,
        AgentId::root(),
        ToolCallId::new("call_card"),
        RemoteToolOutput::Success("{\"cardId\":\"card-1\"}".to_string()),
    );
    assert!(late.is_err(), "超时之后迟到的回传必须被安全拒绝，不能写进已经收尾的轮次");
}
