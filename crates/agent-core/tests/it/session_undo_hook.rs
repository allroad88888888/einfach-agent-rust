//! 199/200 的**公开面**：三态是怎么被真事件写出来的，以及无参 `undo_turn()` 在
//! 有 `Hooked` 条目时的行为。
//!
//! 逐条循环本身（顺序、失败、`/undo!` 额度、逆序）在 `agent-core` 的白盒单测
//! `command/undo_hook_tests.rs` 上钉——那一组要精确控制每条 entry 写了什么值，
//! 走的是 `Session::restore`。这里反过来：**只用公开 API**，钉住宿主真按
//! 「派发时标一下、结果落地时定性」这条路走时，落进日志的档位是对的。

use std::sync::Arc;

use agent_core::{
    AgentId, BlockedCause, ContentBlock, Event, HookOutcome, PrefixImage, Session, StopReason,
    TokenUsage, ToolCallId, UndoReport, Undoability,
};

/// 一次真实的「模型要调一个工具 → 宿主派发前标档位 → 结果落地」序列。
/// `mark` 就是宿主那一下——`agent-runtime` 的 `dispatch` 在真实路径上调的是同两个口。
fn session_after_a_tool_call(mark: fn(&mut Session, ToolCallId)) -> Session {
    let mut session = Session::new(AgentId::root());
    let _ = session.step(Event::UserInput {
        agent: AgentId::root(),
        text: "写个文件".into(),
    });
    let call_id = ToolCallId::new("call_1");
    let _ = session.step(Event::ProviderDone {
        agent: AgentId::root(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::ToolUse {
            id: call_id.clone(),
            name: Arc::from("srv:fs/write"),
            input: Arc::new(serde_json::json!({"path": "a.txt"})),
        }],
        stop: StopReason::ToolUse,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
    mark(&mut session, call_id.clone());
    let _ = session.step(Event::ToolResult {
        agent: AgentId::root(),
        epoch: session.epoch(),
        call_id,
        content: Arc::from("ok"),
    });
    session
}

fn tier(mark: fn(&mut Session, ToolCallId)) -> Undoability {
    session_after_a_tool_call(mark)
        .last_entry()
        .unwrap()
        .meta
        .undoability
}

/// 三条路各自落在哪一档。`mark_hooked` 是 199 §一 加的那条：工具执行完**交回了**
/// 还原函数，于是这一步不是屏障——撤销时去问一次钩子，而不是停下来问用户。
#[test]
fn the_hosts_two_marks_land_on_the_result_entry_and_nothing_else_does() {
    assert_eq!(tier(Session::mark_hooked), Undoability::Hooked);
    assert_eq!(tier(Session::mark_no_undo), Undoability::Blocked);
    // 两样都不标 = 宿主什么都没说 = 这次调用没碰外部世界。
    assert_eq!(tier(|_, _| {}), Undoability::StateOnly);
}

/// **无参 `undo_turn()` 是「递一个恒 `Ok` 的钩子」**：一条 `Hooked` 的 entry 不挡它，
/// 状态照退。这正是「既有调用点一个字节都不用改」的含义——没接钩子表的宿主
/// （wasm / 测试 / 老代码路径）行为跟 199 之前一样。
#[test]
fn the_no_arg_undo_turn_rolls_a_hooked_entry_back_without_asking_anyone() {
    let mut session = session_after_a_tool_call(Session::mark_hooked);
    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "Hooked 不是屏障，无参 undo 该一路退完：{report:?}"
    );
    assert_eq!(session.cursor(), 0);
}

/// 同一条 `Hooked` 的 entry，接上钩子之后：钩子说失败 → 停在它上面，成因是
/// `HookFailed`（**碰了，可能做了一半**），不是屏障的 `NoHook`（**没碰**）。
/// 两种话术不同正是 199 §五 加成因的全部理由。
#[test]
fn the_same_entry_blocks_with_hook_failed_once_a_real_hook_says_so() {
    let mut session = session_after_a_tool_call(Session::mark_hooked);
    let barrier_seq = session.last_entry().unwrap().seq;

    let report = session.undo_turn_with(&mut |_| HookOutcome::Failed(Arc::from("磁盘只读")));
    assert_eq!(
        report,
        UndoReport::Blocked {
            entries: 0,
            barrier_seq,
            cause: BlockedCause::HookFailed(Arc::from("磁盘只读")),
        }
    );

    // 用户确认之后 `/undo!` 越过它——一次确认放行一条，这一轮剩下的照常退完。
    let report = session.undo_turn_force_with(&mut |_| HookOutcome::Failed(Arc::from("磁盘只读")));
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
    assert_eq!(session.cursor(), 0);
}
