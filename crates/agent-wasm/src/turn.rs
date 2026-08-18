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
//!
//! # 但「就地执行」把两件事挪进了这个函数（123）
//!
//! server 形态下宿主执行工具期间泵已经收工、控制权在命令队列上，所以「用户取消」
//! 和「等待槽到点」都由那条队列驱动（`agent-server` 的 `handle_cancel` /
//! `handle_remote_tool_timeout`）。浏览器形态下**没有那条队列**：整轮都在
//! `AgentHost::send` 那一个 Promise 里，工具执行就是链上的一个 `await`。于是这两件
//! 事必须在这里做，否则一条挂住的页面回调 = 整个宿主对页面失去响应（`send()` 握着
//! `live.borrow_mut()`，见 [`crate::host_session`] 的借用纪律）。
//!
//! 打断的判定在 [`crate::interrupt`]（那里也写了「JS Promise 没法真 abort」这件事
//! 选了哪条路），收尾在这里的 [`settle_interrupt`]：**两条出口各自对应一个既有的
//! runtime 入口**，一个新的收尾语义都没有发明。

use agent_core::{Failure, Session, TurnStatus, UndoReport};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolFailure, RemoteToolOutput,
    RemoteToolSubmitOutcome, RemoteToolSubmitRequest, RemoteToolWaiting, ResolveRemoteToolError,
    RunnerCtx, TransientSourceFailure, cancel_pending_remote_tools_async, claim_remote_tool,
    AutoTurnStep, is_transient_source, resolve_remote_tool_async, run_turn_async,
    try_one_auto_turn_async,
    submit_remote_tool_result_async, sweep_remote_tool_deadlines_async,
};

use crate::host_tool;
use crate::interrupt::{self, Interrupted};

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
        // 211：留言自己把下一轮开起来（决策 35 §二）。**逐轮驱动**而不是调
        // `run_auto_turns_async`：这个宿主每一轮之后还要排空 `web:` 工具的等待槽
        // （`drain_host_tools`），而 `agent-runtime` 不认识那件事。
        //
        // **浏览器里「看得见 + 喊得停」比在 CLI 上更要紧**：这儿没有 Ctrl-C。
        // 停的入口是同一个取消标志（`AgentHost::cancel()` 置上，
        // `try_one_auto_turn_async` 在开每一轮之前先看它），面上那两条通报
        // （`auto_turn_started` / `auto_turn_held`）由 `crate::events` 送给页面。
        //
        // `Outcome.status` 仍然是**用户那一轮**的终态，不被自开的轮次改写：
        // 页面靠事件流看自驱动那几轮，靠这个返回值判断刚才那句话的结果。
        while let AutoTurnStep::Ran(auto) = try_one_auto_turn_async(session, ctx).await? {
            // 被取消的那一轮走不到这里：`try_one_auto_turn_async` 自己丢掉半轮
            // （留言退回收件箱）并回 `Held`，循环当场结束。这里只需要把这一轮
            // 里模型发起的 `web:` 工具排空——那件事 `agent-runtime` 不认识。
            let _ = drain_host_tools(session, ctx, auto).await?;
        }
        return Ok(Outcome {
            status,
            cancelled_turn: None,
        });
    }
    // 「取消轮丢弃」的正牌答案是 `Session::undo_turn`（027），不是手工截断消息
    // 列表。跟 `agent_cli::undo::after_cancelled_turn` 同一句；撞上不可逆屏障时
    // 它自己会拒绝，那时半轮内容留着是对的。
    let report = agent_runtime::undo::undo_turn(session, ctx);
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
        let output =
            match interrupt::until_settled(ctx, &waiting, host_tool::execute(&waiting)).await {
                Ok(output) => output,
                Err(interrupted) => match settle_interrupt(session, ctx, interrupted).await? {
                    Some(next) => {
                        status = next;
                        continue;
                    }
                    None => return Ok(status),
                },
            };
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
///
/// 123：执行被打断时这里**不提交**，直接把 [`settle_interrupt`] 的结论当返回值
/// ——它的两种取值跟上面这条约定逐字同款，调用方一行都不用改。认领过的槽由收尾
/// 那两条路各自划掉，截止线因此是从**认领那一刻**起算的（`claim_remote_tool`
/// 会按预算重新起表），不是派发那一刻。
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
    let executed =
        interrupt::until_settled(ctx, &unredacted, host_tool::execute(&unredacted)).await;
    let outcome = match executed {
        Ok(output) => to_submit_outcome(output),
        // 打断之后**不提交**：认领过的槽由收尾那两条路各自划掉（取消 →
        // `discard_remote_tools`；到点 → `take_expired_remote_tools`，因为认领过所以
        // 落的是 `OutcomeUnknown`，而正文被 transient-source 策略换成 `SAFE_ERROR`）。
        // 这里补一次提交只会撞上那条已经收场的槽。
        Err(interrupted) => return settle_interrupt(session, ctx, interrupted).await,
    };
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

/// 一次执行被打断之后，这一轮从哪继续（123）。
///
/// 返回值的约定跟 [`drain_transient_source`] 一致：`Some(status)` = 状态机往前走了
/// 一步，drain 循环拿它当新的 `status` 接着转；`None` = 这一刻没什么可推进的，
/// 调用方把手里的状态原样交回去，**不重试**。
///
/// 两条出口各自复用一个既有的 runtime 入口，一个新语义都没发明：
///
/// - **取消** → [`cancel_pending_remote_tools_async`]：斩断所有等待槽 + 把
///   `Event::Cancel` 喂进转移表。轮次因此落 `Failed(Cancelled)`，[`run`] 那半段照旧
///   走 `undo_turn`（取消轮丢弃），页面拿到 `cancelledTurn`。跟 `agent-server` 的
///   `handle_cancel` 是同一句——浏览器只是没有那条命令队列替它调。
///   槽全被斩断，所以 drain 循环下一圈必然退出。
/// - **到点** → [`sweep_remote_tool_deadlines_async`]：把过期槽翻成一条 `is_error`
///   的工具结果（**带登记那一刻的 epoch**，红线 6 的判据在 runtime 那边，这里不
///   重抄）喂回模型，泵接着把这一轮跑完。模型看得见「这次调用超时了」并自纠，
///   页面看得见一条 `ToolExecuted { is_error: true }`。
///
/// 到点那条的 `Ok(None)`（这一刻其实没有槽过期）结构上不可达——[`interrupt`] 只在
/// **这一条槽**的剩余时间归零时才报 `Expired`，而两边判过期的判据是同一个。
/// 仍然按 `None` 老老实实交回去：真出现了就说明表已经不是我们以为的样子，
/// 那时**再执行一次同一条工具**（副作用做两遍）比停下来糟得多。
async fn settle_interrupt(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    interrupted: Interrupted,
) -> Result<Option<TurnStatus>, TransientSourceFailure> {
    match interrupted {
        Interrupted::Cancelled => cancel_pending_remote_tools_async(session, ctx)
            .await
            .map(Some),
        Interrupted::Expired => sweep_remote_tool_deadlines_async(session, ctx).await,
    }
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
