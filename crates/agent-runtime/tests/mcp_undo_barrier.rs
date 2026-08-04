//! 043 验收第四条：readOnly 的 MCP 工具结果 entry **无屏障位**，`/undo` 干净越过；
//! 非 readOnly 的落屏障，`/undo` 撞它停下推 `undo_blocked`（复用 020/027 的既有屏障
//! 机制，MCP 不新造）。
//!
//! 屏障怎么落：dispatch 第四路先 `snapshot` 再按 `Reversibility::Irreversible` 调
//! `Session::mark_irreversible`——跟 `srv:shell/exec` 那条路一模一样，只是可逆性来自
//! MCP 映射（`readOnlyHint`）而不是名字。所以这条测试和 `shell_exec_undo_barrier`
//! 是同一套断言，换了工具来源。

mod support;

use agent_core::{AgentId, Session, TurnStatus, UndoReport};
use agent_runtime::run_turn;

use support::mcp;

/// 跑一轮：模型调一个 MCP 工具（server 立即回结果）→ hop2 EndTurn → `Done`。
fn run_one_mcp_turn(dir: &std::path::Path, tool: &str, read_only: bool) -> Session {
    let script = mcp::call_script("0", r#"{"content":[{"type":"text","text":"done"}]}"#);
    let wire = format!("mcp_3Aeverything_2F{tool}");
    let port = support::spawn_scripted_server(vec![
        mcp::hop_tool_use(&wire, "call_1"),
        mcp::hop_end_turn(),
    ]);
    let (mut ctx, _events) =
        mcp::build_ctx(port, dir, "everything", vec![mcp::tool_entry("everything", tool, read_only)], &script);
    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "调个 MCP 工具");
    assert_eq!(status, TurnStatus::Done { truncated: false }, "两跳该干净收尾");
    session
}

#[test]
fn read_only_mcp_result_has_no_barrier_and_undo_crosses_it_cleanly() {
    let dir = support::temp_dir("mcp-undo-readonly");
    // readOnly → Pure → 不 mark_irreversible → 结果 entry barrier=false。
    let mut session = run_one_mcp_turn(&dir, "echo", true);

    // `/undo` 一步干净退掉整轮，不撞屏障。
    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "readOnly 该干净越过：{report:?}");
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}

#[test]
fn non_read_only_mcp_result_gets_a_barrier_that_stops_undo_until_forced() {
    let dir = support::temp_dir("mcp-undo-barrier");
    // 非 readOnly → Irreversible → mark_irreversible → 结果 entry barrier=true。
    let mut session = run_one_mcp_turn(&dir, "sendEmail", false);

    // `/undo` 撞上屏障停下（推 undo_blocked），不静默回滚。
    let report = session.undo_turn();
    let UndoReport::Blocked { barrier_seq, .. } = report else {
        panic!("非 readOnly 该撞屏障停下，拿到 {report:?}");
    };
    let barrier_entry = session.history().entries().find(|e| e.seq == barrier_seq).unwrap();
    assert!(barrier_entry.meta.barrier, "撞停的这条 entry 该带 barrier 位");

    // `/undo!` 才越过。
    let report = session.undo_turn_force();
    assert!(matches!(report, UndoReport::Applied { .. }), "强制越过该成功：{report:?}");
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}
