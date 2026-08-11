//! 一整轮对话：喂一句话 → 泵驱动到静止 → 宿主工具在轮内被就地执行 → 再驱动。
//!
//! # `begin_turn` 的时机跟 CLI 逐字一致
//!
//! **只在终态之后才 `begin_turn`**：`Session::new` 与刚恢复出来的会话都可能已经
//! 是 `Idle`，多调一次会把 `turn_id` 平白推进一格；恢复出来卡在非终态
//! （`ToolsPending`/`Thinking`）也不调——那种状态下第一条新输入会被转移表判成
//! 协议违规、状态原样不动，靠 undo 摆脱，不是在这里悄悄开新的一轮。
//! 判据与措辞照抄 `agent_cli::repl::run` 的同一段。
//!
//! # 为什么宿主工具在这里排空，而不是让页面自己回传
//!
//! `web:` 工具走的是 M10 的远端等待槽：`run_turn_async` 会带着一个非终态的
//! `ToolsPending` 返回，等宿主调 `resolve_remote_tool_async`。server 形态下那一步隔着
//! 一次 HTTP 往返，所以要把控制权交还给客户端；浏览器形态下**宿主就是我们
//! 自己**，两端同进程，没有任何理由把一轮对话切成两次 JS 调用——页面说一句话
//! 就该等到一个答案。所以这个函数把「执行 + 回传」就地做掉，页面看到的仍然是
//! 一个 Promise 一轮对话。
//!
//! 循环为什么不会空转：每一圈**消费掉恰好一个**等待槽（`resolve_remote_tool_async`
//! 内部 `take_remote_tool`），而新的槽只可能由模型再要一次工具产生——那需要一次
//! 完整的 provider 往返。所以它既不需要计数上限，也不可能忙等。

use agent_core::{Failure, Session, TurnStatus, UndoReport};
use agent_runtime::{
    RunnerCtx, TransientSourceFailure, resolve_remote_tool_async, run_turn_async,
};

use crate::host_tool;

/// 一轮结束时页面要知道的东西。
pub(crate) struct Outcome {
    /// root 的终态（或者卡住时的非终态）。
    pub(crate) status: TurnStatus,
    /// 这一轮是被取消的话，「取消轮丢弃」那一步的结果。**不丢**：
    /// `UndoReport::Blocked` 说的是「半轮内容因为撞上不可逆屏障而留下了」，
    /// 用户不知道这件事就会以为取消把一切都收拾干净了。
    pub(crate) cancelled_turn: Option<UndoReport>,
}

/// 跑一整轮。
///
/// `Err` 是 M12 那条出口：一次消耗 transient source 的 provider 调用没能收尾，
/// 原始失败事实归嵌入宿主，不进转移表。**这个宿主的工具表里没有任何
/// transient-source 工具**（[`crate::tools`] 只声明两条 `web:`），所以它结构上
/// 不可达；照样把类型带出去而不是就地吞掉，是因为「不可达」是这一版工具表的
/// 性质，不是 API 的性质——工具表哪天多一条，编译器会在这里提醒调用方。
pub(crate) async fn run(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    text: &str,
) -> Result<Outcome, TransientSourceFailure> {
    if session.status().is_terminal() {
        session.begin_turn();
        agent_runtime::persist::sync(ctx, session);
    }
    // 取消标志的清零在 `run_turn_async` 内部做（每轮开始各清一次），这里不重复。
    let status = run_turn_async(session, ctx, text).await?;
    let status = drain_host_tools(session, ctx, status).await;
    if !matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
        return Ok(Outcome {
            status,
            cancelled_turn: None,
        });
    }
    // 「取消轮丢弃」的正牌答案是 `Session::undo_turn`（027），不是手工截断消息
    // 列表。跟 `agent_cli::undo::after_cancelled_turn` 同一句；撞上不可逆屏障时
    // 它自己会拒绝，那时半轮内容留着是对的。
    let report = session.undo_turn();
    agent_runtime::persist::sync(ctx, session);
    Ok(Outcome {
        status,
        cancelled_turn: Some(report),
    })
}

/// 把这一轮里所有派给宿主的 `web:` 调用执行掉并回传，直到没有等待槽为止。
async fn drain_host_tools(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    mut status: TurnStatus,
) -> TurnStatus {
    loop {
        let Some(waiting) = ctx.pending_remote_tools().into_iter().next() else {
            return status;
        };
        let output = host_tool::execute(&waiting);
        match resolve_remote_tool_async(session, ctx, waiting.agent, waiting.call_id, output).await
        {
            Ok(next) => status = next,
            // 回传对不上等待槽（这一轮已经被取消划掉了）——不是可以重试的事，
            // 也不该在这里改状态：把泵最后一次给出的结论原样交回去。
            Err(_) => return status,
        }
    }
}
