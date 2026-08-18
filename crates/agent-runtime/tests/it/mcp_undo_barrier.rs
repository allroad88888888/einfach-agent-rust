//! 043 验收第四条：readOnly 的 MCP 工具结果 entry **无屏障位**，`/undo` 干净越过；
//! 非 readOnly 的落屏障，`/undo` 撞它停下推 `undo_blocked`（复用 020/027 的既有屏障
//! 机制，MCP 不新造）。
//!
//! 屏障怎么落：dispatch 第四路先 `snapshot` 再按 `Reversibility::Irreversible` 调
//! `Session::mark_no_undo`——跟 `srv:shell/exec` 那条路一模一样，只是可逆性来自
//! MCP 映射（`readOnlyHint`）而不是名字。所以这条测试和 `shell_exec_undo_barrier`
//! 是同一套断言，换了工具来源。
//!
//! **202 没有动这两条断言**，值得记一笔：决策 199 §七 的初稿写着「MCP 恒挡」，
//! 那会把 `readOnlyHint: true` 从「不挡」翻成「挡」——**实打实地反转决策 22**，
//! 而 199 从没为它单独论证过。修正后的判据是「承诺挡，事实不挡」：`readOnlyHint:
//! true → Pure` 声明的是「没碰外部世界」这个**事实**，不需要还原函数来兑现，
//! 所以照旧不挡。202 在第四路只补了 `Reversible` 那一格（翻译今天产不出它，
//! 但 `ToolTable::with_mcp` 收得下——见 `undo_promise` 模块文档）。

use crate::support;
use agent_core::{AgentId, Session, TurnStatus, UndoReport};
use agent_runtime::run_turn;

use crate::support::mcp;

/// 跑一轮：模型调一个 MCP 工具（server 立即回结果）→ hop2 EndTurn → `Done`。
fn run_one_mcp_turn(dir: &std::path::Path, tool: &str, read_only: bool) -> Session {
    let script = mcp::call_script("0", r#"{"content":[{"type":"text","text":"done"}]}"#);
    let wire = format!("mcp_3Aeverything_2F{tool}");
    let port = support::spawn_scripted_server(vec![
        mcp::hop_tool_use(&wire, "call_1"),
        mcp::hop_end_turn(),
    ]);
    let (mut ctx, _events) = mcp::build_ctx(
        port,
        dir,
        "everything",
        vec![mcp::tool_entry("everything", tool, read_only)],
        &script,
    );
    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "调个 MCP 工具")
        .expect("MCP call should not be a source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "两跳该干净收尾"
    );
    session
}

#[test]
fn read_only_mcp_result_has_no_barrier_and_undo_crosses_it_cleanly() {
    let dir = support::temp_dir("mcp-undo-readonly");
    // readOnly → Pure → 不 mark_no_undo → 结果 entry `StateOnly`。
    // 202 之后仍然如此：`Pure` 是事实断言，不是承诺（见模块文档）。
    let mut session = run_one_mcp_turn(&dir, "echo", true);

    // `/undo` 一步干净退掉整轮，不撞屏障。
    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "readOnly 该干净越过：{report:?}"
    );
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}

#[test]
fn non_read_only_mcp_result_gets_a_barrier_that_stops_undo_until_forced() {
    let dir = support::temp_dir("mcp-undo-barrier");
    // 非 readOnly → Irreversible → mark_no_undo → 结果 entry `Blocked`。
    let mut session = run_one_mcp_turn(&dir, "sendEmail", false);

    // `/undo` 撞上屏障停下（推 undo_blocked），不静默回滚。
    let report = session.undo_turn();
    let UndoReport::Blocked { barrier_seq, .. } = report else {
        panic!("非 readOnly 该撞屏障停下，拿到 {report:?}");
    };
    let barrier_entry = session
        .history()
        .entries()
        .find(|e| e.seq == barrier_seq)
        .unwrap();
    assert_eq!(
        barrier_entry.meta.undoability,
        agent_core::Undoability::Blocked,
        "撞停的这条 entry 该是屏障"
    );

    // `/undo!` 才越过。
    let report = session.undo_turn_force();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "强制越过该成功：{report:?}"
    );
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}
