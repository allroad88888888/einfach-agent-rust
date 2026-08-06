//! 一轮成功收尾之后的三层判读装配（issue 024 的宿主时序）：
//! `encode → check_drift →（发请求）→ 响应 → reconcile + check_window →
//! GuardReport`——第 1 层已经在 `provider_call::execute` 发请求前做完，
//! 这里只补第 2、3 层，然后把三层拼成一份 [`GuardReport`] 交给回调。

use agent_core::cache::{self, DriftVerdict, GuardReport, ReconcileParams, TurnHit, WindowParams};
use agent_core::{AgentId, TokenUsage};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::execution_binding::GuardScope;

/// 一次成功的 provider 调用之后：对账（第 2 层）+ 滚动窗口（第 3 层，先把
/// 这一轮记进起飞时 binding scope 的 guard history 再算，跟 `agent-core::cache` 模块
/// 文档的示例顺序一致）→ 拼成 `GuardReport` 经回调交给宿主。
///
/// `guard_history` 按 binding scope 隔离：同一 binding 的整棵 agent 树共用一个
/// 窗口，但不同 provider/model 的缓存观测不能混在一起；默认 provider 切换后的
/// 旧请求也只会写回旧 scope。
pub(crate) fn report_success(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    scope: GuardScope,
    usage: &TokenUsage,
    drift: DriftVerdict,
    predicted_cache: u32,
    adjustments: Vec<agent_core::Adjustment>,
) {
    let reconcile = cache::reconcile(predicted_cache, usage.cached, ReconcileParams::default());

    let history = ctx.guard_history_for(scope);
    history.push(TurnHit::from_usage(usage));
    let window = cache::check_window(history, WindowParams::default());

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
