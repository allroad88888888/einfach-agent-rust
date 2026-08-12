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
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolFailure, RemoteToolOutput,
    RemoteToolSubmitOutcome, RemoteToolSubmitRequest, RemoteToolWaiting, ResolveRemoteToolError,
    RunnerCtx, TransientSourceFailure, claim_remote_tool, is_transient_source,
    resolve_remote_tool_async, run_turn_async, submit_remote_tool_result_async,
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
/// 原始失败事实归嵌入宿主，不进转移表。**124 之前这条路结构上不可达**——工具表
/// 里没有任何 transient-source 工具（[`crate::tools`] 只声明两条 `web:page/`）。
/// 124 之后工具表多了 `web:source/echo`（验收脚手架，[`crate::tools::
/// SOURCE_ECHO_TOOL`]），[`drain_host_tools`] 会把它派给
/// [`agent_runtime::submit_remote_tool_result_async`]，那条路**真的会**触发这个
/// `Err`——一次续接 provider 调用没能把 transient-source 那一跳收尾。页面看到
/// 的是：`AgentHost::send` 那个 Promise 被 reject（`host.rs` 已经把它当一条
/// 给页面的错误处理，不是假装成功的终态），不是一条卡死或静默吞掉的调用。
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
    let status = drain_host_tools(session, ctx, status).await?;
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
///
/// 124：按工具名分流。`web:source/` 前缀的 transient-source 工具必须走
/// [`drain_transient_source`]（[`submit_remote_tool_result_async`] 那条正门）——
/// [`resolve_remote_tool_async`] 会显式拒绝它们（`ResolveRemoteToolError::
/// InvalidResult`，见 `agent-runtime` 的 `remote_tool.rs`）。其余工具照旧走
/// [`resolve_remote_tool_async`]，不需要认领（claim）这一整套 CAS 协议——那套
/// 协议是为 transient-source 的幂等重放/指纹记录而存在的，普通工具不需要它多付
/// 这份状态开销。
///
/// 判定用的是 `agent_runtime::is_transient_source`（跨 crate 公开的判定函数），
/// **不是**在这里重抄一份 `"web:source/"` 字面量——两份前缀常量哪天被改歪一个，
/// 症状是安全策略静默失效：入参和结果照常进历史，不报错。见 124 的实做记录。
async fn drain_host_tools(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    mut status: TurnStatus,
) -> Result<TurnStatus, TransientSourceFailure> {
    loop {
        let Some(waiting) = ctx.pending_remote_tools().into_iter().next() else {
            return Ok(status);
        };
        if is_transient_source(&waiting.request.tool) {
            status = match drain_transient_source(session, ctx, waiting).await? {
                Some(next) => next,
                // 认领失败或回传被拒（等待槽已经不是「还欠着」的那一个）——不是
                // 可以重试的事，也不该在这里改状态：把泵最后一次给出的结论原样
                // 交回去，跟下面 `resolve_remote_tool_async` 的失配处置同一处置。
                None => return Ok(status),
            };
            continue;
        }
        let output = host_tool::execute(&waiting);
        match resolve_remote_tool_async(session, ctx, waiting.agent, waiting.call_id, output).await
        {
            Ok(next) => status = next,
            // 回传对不上等待槽（这一轮已经被取消划掉了）——不是可以重试的事。
            Err(ResolveRemoteToolError::InvalidResult(_)) => return Ok(status),
            // 结构上今天也不可达（这条分支只在续接 provider 调用时才可能触发，
            // 而普通工具的续接不会去动 transient-source 的收尾），但类型上存在
            // 就必须处理：原样冒泡给 `run`，别悄悄吞掉一次真失败。
            Err(ResolveRemoteToolError::TransientSource(failure)) => return Err(failure),
        }
    }
}

/// 认领并回传一次 `web:source/` 调用。
///
/// 认领（[`claim_remote_tool`]）不是协议里可选的一步：等待槽投影
/// （`ctx.pending_remote_tools()`）里的 `request.input` 对 transient-source
/// 工具**永远是 dispatch 派发时脱敏过的占位符**（`{"transient_source":
/// "redacted"}`），只有认领成功拿到的 `RemoteToolClaimGrant::request` 才是
/// 未脱敏的真入参——脱敏只保护历史/prompt，不能连执行这一步也一起挡住，否则
/// `web:source/echo` 这类脚手架只会把「redacted」回显给模型，验证不了任何事。
///
/// 单进程宿主永远是唯一认领者：`Claimed`/`AlreadyClaimedByYou` 之外的判定
/// （被别人抢占、终态已回执、回执过期、查无此调用）在这个宿主里结构上不可达，
/// 但仍按类型完整匹配——出现了就说明等待槽已经不是这一轮该管的那个，安全地
/// 放弃这一槽（返回 `Ok(None)`，调用方原样交回当前状态，不重试）。
///
/// `Ok(None)` 表示这次回传没有让状态机往前走（认领失败，或提交本身被判定为
/// 重放/冲突/未认领而没有新事件提交）；`Ok(Some(status))` 才是真的推进了一步。
async fn drain_transient_source(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    waiting: RemoteToolWaiting,
) -> Result<Option<TurnStatus>, TransientSourceFailure> {
    let RemoteToolWaiting { agent, call_id, .. } = waiting;
    let claim_id = format!("wasm-drain-claim:{}", call_id.0);
    let claim = claim_remote_tool(
        session,
        ctx,
        RemoteToolClaimRequest {
            agent: agent.clone(),
            call_id: call_id.clone(),
            claim_id: claim_id.clone(),
        },
    );
    let grant = match claim {
        RemoteToolClaimDecision::Claimed(grant)
        | RemoteToolClaimDecision::AlreadyClaimedByYou(grant) => grant,
        RemoteToolClaimDecision::ClaimedByOther { .. }
        | RemoteToolClaimDecision::Terminal(_)
        | RemoteToolClaimDecision::StatusNotRetained
        | RemoteToolClaimDecision::UnknownToolCall => return Ok(None),
    };
    let unredacted = RemoteToolWaiting {
        agent: agent.clone(),
        call_id: call_id.clone(),
        request: grant.request,
    };
    let outcome = to_submit_outcome(host_tool::execute(&unredacted));
    submit_remote_tool_result_async(
        session,
        ctx,
        RemoteToolSubmitRequest {
            agent,
            call_id: call_id.clone(),
            claim_id,
            submission_id: format!("wasm-drain-submit:{}", call_id.0),
            outcome,
        },
        // 决策本身不用于任何分支：`submission_id` 是刚按 `call_id` 现铸的，
        // 单进程宿主也不会有并发的第二次提交，`Committed` 是唯一能真的走到
        // 这里的分支。忽略回执细节是安全的——它不是 API 要求调用方看的东西。
        |_decision| {},
    )
    .await
}

/// [`host_tool::execute`] 的 [`RemoteToolOutput`] → [`RemoteToolSubmitOutcome`]。
/// 两种协议形状不同只是因为 `resolve_remote_tool_async`（简单二选一）和
/// `submit_remote_tool_result_async`（要带结构化失败信息以便将来分类重试）
/// 各自服务不同的调用方，跟工具执行本身的语义无关——这里就是个纯搬运。
fn to_submit_outcome(output: RemoteToolOutput) -> RemoteToolSubmitOutcome {
    match output {
        RemoteToolOutput::Success(content) => RemoteToolSubmitOutcome::Succeeded { content },
        RemoteToolOutput::Failure(message) => RemoteToolSubmitOutcome::Failed {
            error: RemoteToolFailure {
                code: "host_tool_failed".to_string(),
                message,
                retryable: false,
                details: None,
            },
        },
    }
}
