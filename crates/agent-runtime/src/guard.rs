//! 一轮成功收尾之后的三层判读装配（issue 024 的宿主时序）：
//! `encode → check_drift →（发请求）→ 响应 → reconcile + check_window →
//! GuardReport`——第 1 层已经在 `provider_call::execute` 发请求前做完，
//! 这里只补第 2、3 层，然后把三层拼成一份 [`GuardReport`] 交给回调。

use agent_core::cache::{self, DriftVerdict, GuardReport, ReconcileParams, TurnHit, WindowParams};
use agent_core::{AgentId, TokenUsage};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;

/// 一次成功的 provider 调用之后：对账（第 2 层）+ 滚动窗口（第 3 层，先把
/// 这一轮记进 [`RunnerCtx::guard_history`] 再算，跟 `agent-core::cache` 模块
/// 文档的示例顺序一致）→ 拼成 `GuardReport` 经回调交给宿主。
///
/// `guard_history`（第 3 层的滚动窗口）是**整棵树共用一份**：它记的是「这个
/// 会话最近几轮的缓存命中观测」，而一次会话对 provider 的用量本来就是全树合起来
/// 的那一笔账。按 agent 分窗会让每个短命子 agent 各自攒一条永远够不到窗口宽度的
/// 序列，第 3 层对谁都失效——029 的多 agent 没有改变这一层要回答的问题。
pub(crate) fn report_success(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    usage: &TokenUsage,
    drift: DriftVerdict,
    predicted_cache: u32,
    adjustments: Vec<agent_core::Adjustment>,
) {
    let reconcile = cache::reconcile(predicted_cache, usage.cached, ReconcileParams::default());

    ctx.guard_history.push(TurnHit::from_usage(usage));
    let window = cache::check_window(&ctx.guard_history, WindowParams::default());

    let report = GuardReport {
        drift,
        reconcile,
        window,
    };
    ctx.emit(
        agent,
        RunnerEvent::TurnGuard {
            usage: usage.clone(),
            report,
            adjustments,
        },
    );
}
