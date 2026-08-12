//! 截止线：谁到点了，到点变成哪条事件。
//!
//! 泵里有两类「在等一个也许永远不来的东西」，各有各的截止线。到点都会把凭据从
//! 表里划掉，换成一条事件喂回转移表，让 core 决定重试还是收敛；provider 调用还
//! 会先锁存它独立的取消标志，使本地准备/上传等待/流读取及时收敛：
//!
//! | 在等什么 | 凭据 | 到点注入 |
//! |---|---|---|
//! | provider 的一次流式响应 | [`ProviderCall`]（泵的 `calls` 表，012） | `Event::Timeout` |
//! | 远端宿主的一次 `POST /tool_result` | [`crate::ctx_remote_tools::PendingRemoteTool`]（060） | `Event::ToolFailed` |
//!
//! # 为什么远端那一条还要一个**宿主侧**入口
//!
//! provider 在飞时泵是活的（它在 `recv_timeout` 上转圈），到点自然扫得到。远端
//! 等待不是：`Dispatched::Nothing` 之后泵这一圈就撞上「在飞表空了」收工返回
//! `ToolsPending`，控制权回到宿主的命令队列——**回传本身就是从那条队列进来的**
//! （`agent-server` 的 `Command::RemoteToolResult`），泵要是赖着不走等回传，那条
//! 命令永远没人收，当场死锁。
//!
//! 所以远端截止线必须由**空闲时握着控制权的那一方**驱动：宿主问
//! [`RunnerCtx::next_remote_deadline`] 该等多久，等超了调
//! [`sweep_remote_tool_deadlines`]。[`sweep`]（泵里那一份）覆盖的是另一半世界
//! ——root 在等远端、同时后台子 agent 还在飞，泵这时本来就活着，到点顺手扫掉，
//! 不必等它收工。两半共用 [`expired`]，产出的事件逐字节同款。

use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentId, Event, Session, ToolCallId, TurnStatus};
// 114b：`Instant::now()` panic 在 wasm32-unknown-unknown 上，垫 `web-time`
// （native 目标下就是 `std::time::Instant` 本尊，行为不变）。
use web_time::Instant;

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::provider_call::ProviderCall;
use crate::runner;
use crate::transient_source_failure::TransientSourceFailure;

/// 泵里的一次截止线扫描：到点的 provider 调用和到点的远端等待都翻成待办事件。
///
/// 每个在飞调用各有各的截止线（它们不是同时起飞的）。provider 到点时先锁存
/// call-local 取消再划掉凭据；不 join，也不承诺物理中断已经进入底层的阻塞请求，
/// 具体边界见 `provider_call` 模块文档。
pub(crate) fn sweep(
    ctx: &mut RunnerCtx,
    calls: &mut Vec<ProviderCall>,
    pending: &mut std::collections::VecDeque<Event>,
) -> Option<TransientSourceFailure> {
    let now = Instant::now();
    let mut i = 0;
    while i < calls.len() {
        if calls[i].deadline > now {
            i += 1;
            continue;
        }
        let call = calls.remove(i);
        // Removing the credential abandons the result; latch cancellation as well so request
        // preparation releases its leases and cannot continue to another upload or chat call.
        call.cancel();
        if call.one_shot {
            ctx.transient_sources
                .purge_agent_epoch(&call.agent, call.epoch);
            return Some(TransientSourceFailure::ProviderDeadlineExceeded {
                agent: call.agent,
                epoch: call.epoch,
            });
        } else {
            pending.push_back(Event::Timeout {
                agent: call.agent,
                epoch: call.epoch,
                call_id: None,
            });
        }
    }
    pending.extend(expired(ctx, now));
    None
}

/// 宿主侧入口：空闲等命令等到远端截止线了，调它一次（060）。
///
/// `Ok(None)` = 这一刻没有任何槽过期（提前醒了 / 竞态里刚好被真回传收敛了），宿主
/// 照旧回去等命令。`Ok(Some(status))` = 至少一个槽被判失败并驱动了事件泵，返回的是
/// 泵停下来时的轮次状态，宿主按自己那套处理；`Err` 保留 transient-source provider
/// 调用的原始失败事实（`agent-server` 的
/// `commands::handle_remote_tool_timeout`）。
///
/// 多个槽同时过期时逐个恢复：每次 [`runner::resume_async`] 把泵驱动到静止，剩下
/// 的槽让下一次继续——`Session::step` 一次只吃一条事件，攒成一批喂进去也是同一
/// 条路。
///
/// 116：`async fn`，因为循环体里的 `runner::resume_async` 本身就是 await 链的
/// 一环；逐个 `.await` 而不是并发等待——语义跟改动前逐个同步调用完全一致，槽位
/// 过期的处理顺序不该被并发打乱。native 上的同步入口见
/// [`sweep_remote_tool_deadlines`]。
pub async fn sweep_remote_tool_deadlines_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<Option<TurnStatus>, TransientSourceFailure> {
    let events = expired(ctx, Instant::now());
    let mut status = None;
    for event in events {
        status = Some(runner::resume_async(session, ctx, event).await?);
    }
    Ok(status)
}

/// 指定等待槽**还剩多久到点**；`None` = 表里没有这条（已经收敛 / 已经被斩断）。
///
/// 123 加的，服务于一种 060 当时不存在的宿主形态：浏览器里执行一条 `web:` 工具是
/// **就地 `await` 页面的一个 Promise**，没有「回去等命令」那一步可等，所以宿主要把
/// 那次 await 本身变成可打断的等待——它需要的正是这个数。
///
/// # 为什么不能用 [`RunnerCtx::next_remote_deadline`] 顶替
///
/// 那个给的是**全表最早**的一条（server 形态下宿主空闲阻塞多久要的正是它）。同一
/// 批派出的多个调用截止线只差微秒，但「全表最早的到点了」不等于「我手里正在执行的
/// 这条到点了」：拿前者判后者，会在 B 到点而 A 还没到点时把 A 那次**正在正常执行**
/// 的调用丢掉，槽还留在表里，下一圈又把同一条工具执行一遍。副作用执行两次不报错。
///
/// # 为什么返回 `Duration` 而不是 `Instant`
///
/// 把时钟读取留在这个 crate 里。114b/`dd23637` 已经把 `Instant`/`SystemTime` 统一垫
/// 成 `web-time` 并加了 `tests/it/wasm_clock_source.rs` 那条守卫；调用方拿到的是一段
/// **相对时长**，不必自己再取一次时间，也就不会在别处冒出第三种取时间的方式。
///
/// `Some(Duration::ZERO)` = 已经到点，且这一刻调 [`sweep_remote_tool_deadlines_async`]
/// 必然扫得到它：两边判过期用的是同一条判据（`deadline <= now`），而时间只会往前走。
pub fn remote_tool_deadline_in(
    ctx: &RunnerCtx,
    agent: &AgentId,
    call_id: &ToolCallId,
) -> Option<Duration> {
    let now = Instant::now();
    ctx.pending_remote_tools
        .pending
        .iter()
        .find(|pending| &pending.agent == agent && &pending.call_id == call_id)
        .map(|pending| pending.deadline.saturating_duration_since(now))
}

/// [`sweep_remote_tool_deadlines_async`] 的同步壳。理由与 `cfg` 的取舍见
/// `crate::runner` 模块文档「但公开入口在 native 上仍然是同步的」。
#[cfg(not(target_arch = "wasm32"))]
pub fn sweep_remote_tool_deadlines(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<Option<TurnStatus>, TransientSourceFailure> {
    crate::block_on(sweep_remote_tool_deadlines_async(session, ctx))
}

/// 到点的远端等待槽 → 一条 `is_error` 的工具结果事件。
///
/// **epoch 用登记那一刻的那个，不是「现在的」**（红线 6）。到点这一刻世代可能
/// 已经被 undo/取消推走了，那正是这条红线要挡的世界：拿当前 epoch 组事件等于
/// 亲手把一个幽灵结果送过 `Session::step` 的闸，写进一个已经回滚掉的世界——
/// 而且不报错。这跟正常回传（`crate::remote_tool::resolve_remote_tool` 用
/// `pending.epoch`）是同一份判据，超时不许有第二套。
///
/// 顺带发一条 `ToolExecuted { is_error: true }`：宿主/UI 看到的是「这次调用有
/// 了结局」，跟真回传落地时同款，不需要为超时新造一种可见性。
fn expired(ctx: &mut RunnerCtx, now: Instant) -> Vec<Event> {
    let budget = ctx.remote_tool_timeout;
    ctx.take_expired_remote_tools(now)
        .into_iter()
        .map(|pending| {
            let transient = crate::transient_source_policy::is_transient_source(
                &pending.request.tool,
            );
            let error = if transient {
                crate::transient_source_policy::SAFE_ERROR.to_owned()
            } else if pending.claim_id.is_some() {
                format!(
                    "[remote_tool_outcome_unknown] 远端工具结果超时：宿主已领取 {}，但在 {}s 内没有回传结果",
                    pending.request.tool,
                    budget.as_secs_f64(),
                )
            } else {
                // 措辞刻意**不说**「宿主没有领取」。领取（`claim_remote_tool`）只在
                // 拉取式宿主那条路上是必经步骤；同进程宿主（浏览器，M14）执行普通
                // `web:` 工具时根本不认领，于是 `claim_id` 恒为 `None`——**哪怕它
                // 正在执行**。旧文案在浏览器形态下字面为假，而且 123 的真机验收里
                // 模型照着它向用户复述了一遍「页面端宿主没有领取这次调用」。
                //
                // 现在这句在两种形态下都成立：拉取式宿主没来领 = 没回传；同进程宿主
                // 挂住了 = 也没回传。**说「没回传」是可观测事实，说「没领取」是对
                // 原因的猜测**，而这段文字是给模型看的。
                format!(
                    "[remote_tool_timeout][remote_tool_unclaimed_timeout] 远端工具超时：{}s 内没有收到 {} 的结果，这次调用按失败收尾",
                    budget.as_secs_f64(),
                    pending.request.tool,
                )
            };
            ctx.emit(
                &pending.agent,
                RunnerEvent::ToolExecuted {
                    call_id: pending.call_id.clone(),
                    tool: Arc::clone(&pending.request.tool),
                    output_len: error.len(),
                    is_error: true,
                },
            );
            Event::ToolFailed {
                agent: pending.agent,
                epoch: pending.epoch,
                call_id: pending.call_id,
                error: Arc::from(error),
            }
        })
        .collect()
}
