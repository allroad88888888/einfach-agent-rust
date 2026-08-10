//! Issue 100 验收第一条，走**真实生产路径**：`Session::replace_send_plan` 之后，
//! 真的触发一轮 `run_turn`，用录制服务器截获实际发到假上游的请求体——不是测试
//! 自己手搓 `project()`+`encode()` 的组合（那条在 `agent-providers` 的
//! `send_plan_clearing.rs` 已经测过），是证明 `provider_call::start` 这条生产
//! 代码路径真的从 `Session` 读了 `SendPlan`、算了投影、把结果交给了 `encode`。
//!
//! 三轮各自真读一个不同的文件（`srv:fs/read` 是真实执行的工具，输出是磁盘上
//! 的真内容），产出三个**真实**的 `ToolCallId`；清掉这三个之后触发第四轮，
//! 断言那次录到的请求体里三份工具结果正文都不见了、占位文本出现三次，同时
//! 前三轮的用户提问原样还在——证明清的是工具结果，不是整段历史。

use agent_core::{AgentId, CLEARED_TOOL_RESULT, ContentBlock, SendPlan, Session, ToolCallId};
use agent_runtime::run_turn;

use crate::support;

#[test]
fn clearing_three_tool_results_disappears_from_the_next_real_request_body() {
    let dir = support::temp_dir("send-plan-clears-wiring");
    std::fs::write(dir.join("alpha.txt"), b"ALPHA-CONTENT-ONE").unwrap();
    std::fs::write(dir.join("beta.txt"), b"BETA-CONTENT-TWO").unwrap();
    std::fs::write(dir.join("gamma.txt"), b"GAMMA-CONTENT-THREE").unwrap();

    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_tool_call("call_alpha", "srv_3Afs_2Fread", r#"{\"path\": \"alpha.txt\"}"#),
        support::sse_text("读完 alpha 了"),
        support::sse_tool_call("call_beta", "srv_3Afs_2Fread", r#"{\"path\": \"beta.txt\"}"#),
        support::sse_text("读完 beta 了"),
        support::sse_tool_call("call_gamma", "srv_3Afs_2Fread", r#"{\"path\": \"gamma.txt\"}"#),
        support::sse_text("读完 gamma 了"),
        support::sse_text("好的，三份都总结完毕"),
    ]);
    let (mut ctx, _events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());

    run_turn(&mut session, &mut ctx, "读一下 alpha.txt，暗号 ALPHAQ")
        .expect("第一轮不该是 source failure");
    session.begin_turn();
    run_turn(&mut session, &mut ctx, "读一下 beta.txt，暗号 BETAQ")
        .expect("第二轮不该是 source failure");
    session.begin_turn();
    run_turn(&mut session, &mut ctx, "读一下 gamma.txt，暗号 GAMMAQ")
        .expect("第三轮不该是 source failure");

    // 三个真实的 ToolCallId，从会话历史里取出来，不是自己现造的假 id。
    let root = AgentId::root();
    let mut ids = Vec::new();
    for message in session.messages_of(&root).iter() {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, .. } = block {
                ids.push(id.clone());
            }
        }
    }
    assert_eq!(ids.len(), 3, "三轮各一次工具调用，该有三个 ToolCallId：{ids:?}");
    assert_eq!(ids, vec![
        ToolCallId::new("call_alpha"),
        ToolCallId::new("call_beta"),
        ToolCallId::new("call_gamma"),
    ]);

    let mut plan = SendPlan::new();
    plan.clear_tool_results(ids.iter().cloned());
    session.begin_turn();
    session.replace_send_plan(&root, plan);

    run_turn(&mut session, &mut ctx, "把前面都总结一下")
        .expect("第四轮不该是 source failure");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 7, "三轮各两跳 + 第四轮一跳 = 7 次请求");
    let last = &bodies[6];

    for gone in ["ALPHA-CONTENT-ONE", "BETA-CONTENT-TWO", "GAMMA-CONTENT-THREE"] {
        assert!(
            !last.contains(gone),
            "被清掉的工具结果正文不该出现在第四轮的真实请求体里：{gone}\n{last}"
        );
    }
    assert_eq!(
        last.matches(CLEARED_TOOL_RESULT).count(),
        3,
        "占位文本该出现恰好 3 次：{last}"
    );

    // 没被清的东西还在：三轮各自的用户提问原样保留，证明清的是工具结果的
    // 正文，不是整段历史。
    for marker in ["ALPHAQ", "BETAQ", "GAMMAQ"] {
        assert!(
            last.contains(marker),
            "没被清掉的用户提问该原样留在第四轮请求体里：{marker}\n{last}"
        );
    }
}
