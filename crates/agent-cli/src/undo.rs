//! `/undo` `/redo` `/undo!`（issue 027）+ 取消轮的自动擦除策略。
//!
//! 三条斜杠命令 turn 粒度（决策 5 的默认档），全部先调 `Session` 的命令、再
//! [`agent_runtime::persist::sync`] 把游标变化转发进持久化后端——CLI 直接调用的
//! 会话命令跟 `run_turn` 内部的 `session.step` 一样，都要经这一步（`sync` 模块
//! 文档：调用方必须在每次会话命令之后同步）。
//!
//! `/undo!` 的措辞要让用户明白自己在确认什么：[`describe_barrier`] 从被越过
//! 那条 entry 里把工具名 + call_id 抠出来，不是甩一个 `barrier_seq` 数字给用户
//! 猜——034 起这段抠取逻辑搬进了 `agent_core::Session::barrier_info`（CLI 与
//! `agent-server` 的 `UndoOutcome::Blocked` 富化共用同一个读口），这里只剩「把
//! `BarrierInfo` 格式化成一句人话」。

use agent_core::{BlockedCause, Session, UndoReport, Undoability};
use agent_runtime::RunnerCtx;

use crate::print;

/// `/undo`：撤一整轮。撞上屏障就停下打 `undo_blocked`，不静默回滚。
pub fn undo(session: &mut Session, ctx: &mut RunnerCtx) {
    let report = session.undo_turn();
    agent_runtime::persist::sync(ctx, session);
    report_undo(session, report, false);
}

/// `/redo`：反演一次 undo。redo 没有屏障（`Session::redo_turn` 的文档：只是把
/// 值写回去，不重放外部副作用），所以只有 `Applied`/`Nothing` 两种结果。
pub fn redo(session: &mut Session, ctx: &mut RunnerCtx) {
    let report = session.redo_turn();
    agent_runtime::persist::sync(ctx, session);
    match report {
        UndoReport::Applied { entries, turn_id } => print::redo_applied(entries, turn_id),
        UndoReport::Nothing => print::redo_nothing(),
        UndoReport::Blocked { .. } => {
            unreachable!("redo 没有屏障，Session::redo_turn 的文档写明了")
        }
    }
}

/// `/undo!`：越过**第一条**屏障再退（`Session::undo_turn_force` 只放行一条，
/// 同一轮里第二个不可逆操作还是会再停一次）。
pub fn undo_force(session: &mut Session, ctx: &mut RunnerCtx) {
    let cursor_before = session.cursor();
    let report = session.undo_turn_force();
    agent_runtime::persist::sync(ctx, session);
    report_undo(session, report.clone(), true);
    // 成功越过：把 [cursor_after, cursor_before) 这一段里带 barrier 的 entry
    // 找出来，明确告诉用户越过了什么——`UndoReport::Applied` 本身不带这个信息。
    if let UndoReport::Applied { .. } = report {
        let crossed: Vec<String> = session
            .history()
            .entries()
            .skip(session.cursor())
            .take(cursor_before - session.cursor())
            // 只找屏障：`Hooked` 那一档没被「越过」，它是**还原函数真的跑过了**，
            // 说「已越过 xxx，副作用不会被回滚」对它是句假话（199 的三态之后
            // 这一处机械跟随会改错，所以判据写死在 `Blocked` 上）。
            .filter(|e| e.meta.undoability == Undoability::Blocked)
            .map(|e| describe_barrier(session, e.seq))
            .collect();
        for what in crossed {
            print::undo_force_crossed(&what);
        }
    }
}

fn report_undo(session: &Session, report: UndoReport, forced: bool) {
    match report {
        UndoReport::Applied { entries, turn_id } => print::undo_applied(entries, turn_id),
        UndoReport::Nothing => print::undo_nothing(),
        UndoReport::Blocked {
            entries,
            barrier_seq,
            cause,
        } => {
            let what = describe_barrier(session, barrier_seq);
            print::undo_blocked(entries, &what, &describe_cause(&cause), forced);
        }
    }
}

/// 三种成因 → 三句不同的人话（199 §五：屏障是「**没碰**」，后两种是「**碰了，
/// 而且可能做了一半**」）。用户据此决定要不要 `/undo!`——同一句「撞上了不可逆
/// 操作」套在还原失败上会让他以为外部世界还是干净的。
fn describe_cause(cause: &BlockedCause) -> String {
    match cause {
        BlockedCause::NoHook => "它没有提供还原函数，本仓无从代它回退".to_string(),
        BlockedCause::HookFailed(why) => {
            format!("它的还原函数跑了但失败了（{why}），可能只还原了一半")
        }
        BlockedCause::HookLost => {
            "它的还原函数随进程重启消失了（函数是闭包，不跨进程），没人能代它回退".to_string()
        }
    }
}

/// 取消轮结束时的自动策略（027 已裁决）：**非 force** 的 `undo_turn`——
/// `Applied` 就是干净擦除，`Blocked` 就是「这一轮已经执行过不可逆工具，保留 +
/// 打印说明」（诚实优于整洁：不替用户悄悄越过一个他没被问到的不可逆操作）。
pub fn after_cancelled_turn(session: &mut Session, ctx: &mut RunnerCtx) {
    let report = session.undo_turn();
    agent_runtime::persist::sync(ctx, session);
    match report {
        UndoReport::Applied { entries, turn_id } => print::cancelled_turn_erased(entries, turn_id),
        UndoReport::Blocked {
            entries,
            barrier_seq,
            cause,
        } => {
            let what = describe_barrier(session, barrier_seq);
            print::cancelled_turn_kept(entries, &what, &describe_cause(&cause));
        }
        UndoReport::Nothing => {} // 没有可退的（理论上不会发生：取消前至少有一条 user_input entry）
    }
}

/// [`agent_core::Session::barrier_info`] 的结果 → 一句人话。`None`（`seq` 不在
/// history 里，理论上不该发生）退回一个只带 seq 的兜底；有 entry 但抠不出工具名/
/// call_id（理论上也不该发生，`barrier` 只会在 tool_result/tool_failed 那条上）
/// 退回带 seq/label 的兜底。
fn describe_barrier(session: &Session, seq: u64) -> String {
    let Some(info) = session.barrier_info(seq) else {
        return format!("entry #{seq}");
    };
    match (info.tool, info.call_id) {
        (Some(tool), Some(call_id)) => format!("{tool}（call_id={}）", call_id.0),
        _ => format!("entry #{seq}（{}）", info.label),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{
        AgentId, ContentBlock, Event, PrefixImage, StopReason, TokenUsage, ToolCallId,
    };

    use super::*;

    /// 一个真实的「派发一次 `srv:shell/exec`、宿主标记不可逆、结果落地」序列
    /// ——跟 `agent-runtime::runner::run_effect` 派发工具时做的事一样（先
    /// `mark_irreversible` 再等结果），只是这里手工喂事件，不需要真的起进程。
    fn session_with_a_barrier_entry() -> Session {
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: "跑个命令".into(),
        });
        let call_id = ToolCallId::new("call_shell_1");
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![ContentBlock::ToolUse {
                id: call_id.clone(),
                name: Arc::from("srv:shell/exec"),
                input: Arc::new(serde_json::json!({"cmd": "echo hi"})),
            }],
            stop: StopReason::ToolUse,
            usage: TokenUsage {
                prompt: 10,
                completion: 5,
                cached: None,
            },
            prefix: PrefixImage {
                segments: Vec::new(),
                prompt_tokens: None,
            },
            adjustments: Vec::new(),
        });
        session.mark_irreversible(call_id.clone());
        let _ = session.step(Event::ToolResult {
            agent: AgentId::root(),
            epoch: session.epoch(),
            call_id,
            content: Arc::from("hi\n"),
        });
        session
    }

    /// `/undo!` 的措辞要点名越过了哪个工具——这条测试钉住 `describe_barrier`
    /// 真的能从一条 barrier entry 里抠出工具名和 call_id，不是编一句空话。
    /// 抠取本身的逻辑已经搬进 `agent_core::Session::barrier_info`（034），这里
    /// 钉的是「CLI 换用公共读口之后，格式化文案没有跑偏」。
    #[test]
    fn describe_barrier_extracts_the_tool_name_and_call_id() {
        let session = session_with_a_barrier_entry();
        let entry = session.last_entry().unwrap();
        assert_eq!(
            entry.meta.undoability,
            Undoability::Blocked,
            "标记过 mark_irreversible，这条 entry 该是屏障"
        );

        let described = describe_barrier(&session, entry.seq);
        assert!(described.contains("srv:shell/exec"), "{described}");
        assert!(described.contains("call_shell_1"), "{described}");
    }

    /// 屏障机制本身（`undo_turn` 停、`undo_turn_force` 越过）已经在
    /// `agent-core` 那一侧钉过（`session_undo_redo.rs`），这里只再确认一次
    /// `srv:shell/exec` 这个真实工具名走的是同一条机制，衔接不掉链子。
    #[test]
    fn undo_turn_stops_at_the_barrier_and_undo_turn_force_crosses_it() {
        let mut session = session_with_a_barrier_entry();

        let report = session.undo_turn();
        assert!(matches!(report, UndoReport::Blocked { .. }), "{report:?}");

        let report = session.undo_turn_force();
        assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
    }
}
