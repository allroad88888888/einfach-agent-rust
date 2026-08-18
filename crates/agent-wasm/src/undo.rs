//! 浏览器宿主的撤销/重做（issue 196）：三个会话命令 + `UndoReport` 的 JSON 化。
//!
//! **能力一直都在，缺的只是这一层。** `Session::undo_turn` 在本 crate 里早就被调用
//! 了一处——[`crate::turn::run`] 的「取消轮丢弃」（027），但那是内部路径，页面够不着。
//! 169 的真机复验发现了这个缺口：CLI 和 `agent-server` 都有完整的 `/undo`，
//! 只有浏览器这一路没接出来。
//!
//! 做法照抄 `agent_cli::undo` 的骨架：**先调 `Session` 的命令、再
//! [`agent_runtime::persist::sync`]**。那一步不是可选的——`sync` 的模块文档写明
//! 调用方必须在每次会话命令之后同步，漏了它撤销就只活在内存里，刷新一次就回来了
//! （而「刷新之后撤掉的轮次不复现」正是本 issue 的验收之一）。
//!
//! # 为什么 `Blocked` 要带上屏障详情
//!
//! 甩一个 `barrier_seq` 数字给页面等于没说。`Session::barrier_info`（034 起是 CLI
//! 与 `agent-server` 共用的读口）能把工具名 + call_id 抠出来，这里照用——
//! 可逆性屏障拦住一次撤销时，用户要能看见**是被什么拦住的**，否则屏障看起来就只是
//! 「undo 有时候不管用」。

use agent_core::{Session, UndoReport};
use agent_runtime::RunnerCtx;

/// `undo`：撤一整轮（决策 5 的默认档）。撞上屏障停下并如实报告，不静默回滚。
///
/// 201 起走 [`agent_runtime::undo`]（带钩子表的那条路）：截获式扩展工具交回的还原
/// 函数在状态回滚**之前**跑（决策 199 §三）。浏览器宿主同样装得了扩展包，所以这条
/// 路对它不是可选的。
pub(crate) fn undo(session: &mut Session, ctx: &mut RunnerCtx) -> String {
    let report = agent_runtime::undo::undo_turn(session, ctx);
    agent_runtime::persist::sync(ctx, session);
    report_json(session, &report)
}

/// `undoForce`：越过**第一条**屏障再退。`Session::undo_turn_force` 只放行一条——
/// 同一轮里第二个不可逆操作还会再停一次，这是有意的（越过永远是一次显式决定，
/// `History` 不记「这条已经问过了」）。
pub(crate) fn undo_force(session: &mut Session, ctx: &mut RunnerCtx) -> String {
    let report = agent_runtime::undo::undo_turn_force(session, ctx);
    agent_runtime::persist::sync(ctx, session);
    report_json(session, &report)
}

/// `redo`：反演一次 undo。redo 没有屏障（`Session::redo_turn` 的文档：只是把值写
/// 回去，不重放外部副作用），所以结果只会是 `Applied`/`Nothing`。
pub(crate) fn redo(session: &mut Session, ctx: &mut RunnerCtx) -> String {
    let report = session.redo_turn();
    agent_runtime::persist::sync(ctx, session);
    report_json(session, &report)
}

/// `UndoReport` → 页面能直接用的 JSON。
///
/// **不用 `format!("{report:?}")`**：`send()` 的 `cancelledTurn` 那样干是因为它只是
/// 一句给人看的附注；这里的结果页面要据此决定显示什么（尤其 `Blocked` 要弹出
/// 「被什么拦住了」），Debug 串会逼页面去解析 Rust 的枚举写法。
fn report_json(session: &Session, report: &UndoReport) -> String {
    let value = match report {
        UndoReport::Applied { entries, turn_id } => serde_json::json!({
            "kind": "Applied",
            "entries": entries,
            "turnId": turn_id,
        }),
        UndoReport::Blocked {
            entries,
            barrier_seq,
            cause,
        } => {
            let info = session.barrier_info(*barrier_seq);
            serde_json::json!({
                "kind": "Blocked",
                "entries": entries,
                "barrierSeq": barrier_seq,
                // `label` 恒有；`tool`/`call_id` 在 core 里是防御性的 Option
                // （barrier 只会落在 tool_result/tool_failed 那条上），照实传 null，
                // 不在这里编一个假名字。
                "barrier": info.map(|i| serde_json::json!({
                    "label": i.label,
                    "tool": i.tool.as_deref(),
                    "callId": i.call_id.map(|id| id.0.to_string()),
                })),
                // 199 §五：`barrier` 回答「停在哪一条」，`cause` 回答「为什么停」。
                // 页面据此换措辞——「没交还原函数」和「还原函数跑挂了、可能做了
                // 一半」对用户是两件事，只给前一句等于把后一种情况说成前一种。
                "cause": cause_json(cause),
            })
        }
        UndoReport::Nothing => serde_json::json!({ "kind": "Nothing" }),
    };
    value.to_string()
}

/// 三种成因 → `{ kind, message? }`。形状跟 `agent-server` 那一侧的
/// `BlockedCause`（邻接标签的 TS 枚举）不必逐字节相同——这条路上没有 ts-rs，
/// 页面读的是这里手拼的 JSON（196 的口子），所以就用本文件其余字段一样的
/// camelCase 风格，把「哪一种 + 一句原因」说全即可。
fn cause_json(cause: &agent_core::BlockedCause) -> serde_json::Value {
    match cause {
        agent_core::BlockedCause::NoHook => serde_json::json!({ "kind": "NoHook" }),
        agent_core::BlockedCause::HookFailed(why) => serde_json::json!({
            "kind": "HookFailed",
            "message": why.as_ref(),
        }),
        agent_core::BlockedCause::HookLost => serde_json::json!({ "kind": "HookLost" }),
    }
}
