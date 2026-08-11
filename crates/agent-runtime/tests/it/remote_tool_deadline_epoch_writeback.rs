//! 060 验收「注意」条（**红线 6，本 issue 唯一违反后不报错的那条**）：超时注入的
//! `ToolFailed` 必须带**登记那一刻**的 epoch，不是「现在的」。
//!
//! # 怎么把静默失败变红
//!
//! 时序焊死：
//!
//! 1. 模型调一个真在表里的远端工具（`browser_action`）→ 登记等待槽，凭据里存下
//!    **登记那一刻的 epoch = 0**；`run_turn` 返回 `ToolsPending`。
//! 2. 用户 `/undo`（`Session::undo_turn`）：这一轮整个回滚，**epoch bump 到 1**。
//!    ——红线 6 的原话就是这个世界：「tool call 在飞时用户按了 undo，结果回来会
//!    写进一个已经被回滚掉的世界」。
//! 3. 截止线到了，`sweep_remote_tool_deadlines` 组出 `ToolFailed`：
//!    - **带登记时的 epoch 0**（正确）→ `Session::step` 入口的闸 `0 != 1` → 丢弃，
//!      一个 primitive 都不写，也不产任何 effect。
//!    - 换成 `session.epoch()`（= 1，突变）→ 过闸 → 落在已经回滚成 `Idle` 的世界
//!      上，`Idle + ToolFailed` 是转移表 25 个非法格之一 → `Effect::Emit(
//!      Notice::ProtocolViolation)` → 泵把它当一条 `RunnerEvent::Notice` 发给宿主。
//!
//! 于是断言「**一条 `ProtocolViolation` 都没有**」就是这条红线的探针：正确实现下
//! 幽灵事件在闸前就死了，宿主什么都听不到；用当前 epoch 则宿主必然听到一声。
//!
//! 断言前先钉住「扫描真的跑了」（返回 `Some`、槽被消费掉），否则这条测试可能
//! 因为「压根没扫到东西」而空过。
//!
//! 真实的 `agent-server` 在 `handle_undo` 里会先 `discard_remote_tools()` 把槽清掉
//! （所以线上走不到这一格）——但红线 6 管的是**回写前的那道闸**，不是上游有没有
//! 顺手清干净：换一个宿主、换一条 undo 入口，这道闸就是最后一层。

use std::time::Duration;

use agent_core::{AgentId, ContentBlock, Notice, Session, TurnStatus, UndoReport};
use agent_runtime::{RunnerEvent, ToolTable, run_turn, sweep_remote_tool_deadlines};

use crate::support::{build_ctx_with, spawn_scripted_server, sse_tool_call, temp_dir};

const BUDGET: Duration = Duration::from_millis(60);

#[test]
fn a_timeout_that_fires_after_an_undo_is_dropped_by_the_epoch_gate() {
    let dir = temp_dir("remote-deadline-epoch");
    // 只挂一跳：undo 之后不该再有任何 provider 调用发生。
    let port = spawn_scripted_server(vec![sse_tool_call(
        "call_card",
        "browser_action",
        r#"{\"action\": \"render_card\"}"#,
    )]);
    let (ctx, events) = build_ctx_with(port, &dir, ToolTable::standard());
    let mut ctx = ctx.with_remote_tool_timeout(BUDGET);
    let mut session = Session::new(AgentId::root());

    let parked = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "渲染一张卡片"));
    assert_eq!(parked, TurnStatus::ToolsPending);
    assert_eq!(
        ctx.pending_remote_tool_count(),
        1,
        "槽得真的在，不然后面扫了个空"
    );
    let registered_epoch = session.epoch();

    // 用户 `/undo`：这一轮回滚，世代被推走。
    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "这一轮该被干净地撤掉：{report:?}"
    );
    assert_ne!(
        session.epoch(),
        registered_epoch,
        "undo 必须 bump epoch——闸才有可比的对象（红线 6）"
    );

    // 到点：扫描真的跑了（返回 Some、槽被消费），产出的事件带的是 epoch 0。
    std::thread::sleep(BUDGET + Duration::from_millis(20));
    let status = agent_runtime::block_on(sweep_remote_tool_deadlines(&mut session, &mut ctx))
        .expect("到点该有槽过期——没有的话这条测试是空的");
    assert_eq!(ctx.pending_remote_tool_count(), 0, "过期槽取走即消费");
    assert!(
        !status.is_terminal(),
        "undo 回到了轮次之前，超时事件被丢弃后状态该还是 Idle：{status:?}"
    );

    let events = events.borrow();
    // 探针：幽灵事件在闸前就死了 —— 宿主一声都没听到。
    let violations: Vec<&RunnerEvent> = events
        .iter()
        .filter(|e| matches!(e, RunnerEvent::Notice(Notice::ProtocolViolation { .. })))
        .collect();
    assert!(
        violations.is_empty(),
        "超时注入的 ToolFailed 用了「现在的」epoch，过闸落进已回滚的世界（红线 6）：{violations:#?}"
    );
    // 同一件事的另一面：回滚掉的世界里没有长出任何工具结果。
    let wrote_ghost = session.messages().iter().any(|m| {
        m.blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
    });
    assert!(
        !wrote_ghost,
        "幽灵结果被写进了已回滚的世界：{:#?}",
        session.messages()
    );
}
