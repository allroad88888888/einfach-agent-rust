//! Issue 100 额外验收一：`replace_send_plan` 之后 `/undo` 一次
//! （`Session::undo_turn`，跟 `undo_after_turns.rs` 记的是同一行），下一轮**真实**
//! 请求的字节回到清除前——逐字节相同。
//!
//! 做法：两个独立 session 各自走一遍**脚本完全相同**的对话——同样的 call_id、
//! 同样的文件内容，因为它们来自脚本而不是运行时铸造的随机值，所以在触发"下一
//! 轮"之前两边的历史逐字节相同。session B 在此之上多做一步
//! `replace_send_plan` → `undo_turn`；如果接线和 undo 都对，B 的下一轮真实请求
//! 字节应该跟从没碰过 `SendPlan` 的 A 完全一样——这条比对本身就是基准，不需要
//! 预先写好"该是什么字节"。

use agent_core::{AgentId, ContentBlock, SendPlan, Session, ToolCallId, UndoReport};
use agent_runtime::run_turn;

use crate::support;

fn hello_txt_scripted() -> Vec<support::ScriptedResponse> {
    vec![
        support::sse_tool_call("call_1", "srv_3Afs_2Fread", r#"{\"path\": \"hello.txt\"}"#),
        support::sse_text("文件读完了"),
        support::sse_text("这是最终总结"),
    ]
}

fn capture_next_round_body(dir: &std::path::Path, touch_send_plan: bool) -> String {
    std::fs::write(dir.join("hello.txt"), b"hello world").unwrap();
    let (port, bodies) = support::spawn_recording_server(hello_txt_scripted());
    let (mut ctx, _events) = support::build_ctx(port, dir);
    let mut session = Session::new(AgentId::root());

    run_turn(&mut session, &mut ctx, "读一下 hello.txt").expect("第一轮不该是 source failure");

    let root = AgentId::root();
    if touch_send_plan {
        let call_id = session
            .messages_of(&root)
            .iter()
            .flat_map(|m| m.blocks.iter())
            .find_map(|block| match block {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .expect("第一轮该产出一个 ToolUse");
        assert_eq!(call_id, ToolCallId::new("call_1"));

        session.begin_turn();
        let mut plan = SendPlan::new();
        plan.clear_tool_results([call_id]);
        session.replace_send_plan(&root, plan);
        assert!(
            !session.send_plan_of(&root).is_pristine(),
            "replace_send_plan 之后该立刻反映在 send_plan_of 上"
        );

        let report = session.undo_turn();
        assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
        assert!(
            session.send_plan_of(&root).is_pristine(),
            "/undo 一次之后该退回 pristine"
        );
    }

    session.begin_turn();
    run_turn(&mut session, &mut ctx, "总结一下").expect("第二轮不该是 source failure");

    let bodies = bodies.lock().unwrap();
    bodies.last().expect("第二轮该有一条被录到的请求体").clone()
}

#[test]
fn replace_send_plan_then_undo_turn_returns_the_next_request_to_pre_clear_bytes() {
    let dir_a = support::temp_dir("send-plan-undo-baseline");
    let dir_b = support::temp_dir("send-plan-undo-cleared-then-undone");

    // A：从没碰过 SendPlan，真正的基准。
    let body_a = capture_next_round_body(&dir_a, false);
    // B：清掉工具结果，立刻 /undo 一次，再触发同样的下一轮。
    let body_b = capture_next_round_body(&dir_b, true);

    assert_eq!(
        body_a, body_b,
        "replace_send_plan 之后 undo 一次，下一轮真实请求字节该跟从没清过的会话逐字节相同"
    );
}
