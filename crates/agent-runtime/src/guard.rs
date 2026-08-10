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
    // **每一轮有观测的都进窗口，压缩轮也不例外。** 曾经短暂地按「压缩轮是预期
    // 全价、不该算进慢性失效」把 `Expected` 那一轮排除掉过（103 早期一条写错的
    // 验收），已回退，理由是那样会开一个正好在灾难场景上的盲区：
    //
    // 压缩要是因为 bug 变成每轮都开火（「每轮改中段、每轮全价」，096 决策记录里
    // 反复点的那个形态），那就是**每一轮都判 `Expected`** → 每一轮都被排除 →
    // 窗口里一条观测都没有 → 第 3 层永远不告警。**唯一能抓这个形态的一层，
    // 恰恰在这个形态下失明。**
    //
    // 一次性的压缩代价本来就已经被容忍了：`DEFAULT_CONSECUTIVE_ALERT` 是 3，
    // `cache/window.rs` 的文档写着「单轮低命中是正常现象（换前缀、压缩、第一次
    // 见这个变体）。连续三轮说明不是一次性代价」。再排除一次就是重复计算这份
    // 容忍。真正只该豁免的是**失明轮**（`TurnHit::Blind`，provider 根本没报
    // `cached`），那由 `TurnHit::from_usage` 自己判，不在这里。
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
