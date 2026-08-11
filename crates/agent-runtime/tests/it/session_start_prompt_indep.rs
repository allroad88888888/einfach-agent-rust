//! 独立测试：issue 135（docs/issues/135-session-start-driver.md）验收里
//! 「进 prompt」那一条——`run_session_start` 产出的前缀块，真的经 `run_turn`
//! 到达发给 provider 的 wire 请求体的 system 段，且逐轮字节稳定（红线 11）。
//! 只依据「验收」/「注意」两节 + `docs/INVARIANTS.md` 红线 11 写成，**不看**
//! `crates/agent-runtime/src/session_start.rs` / `subagent.rs` / `runner.rs`
//! 里的实现体，也不看 `provider_call.rs` 怎么组 `Ingredients`。
//!
//! `run_session_start` 自己的契约（顺序/空文本/失败/恢复不重跑）在
//! `session_start_indep.rs` 里，那四条都不碰 provider/wire，本文件只管这一条。
//!
//! **选路**：走既有 fake-provider 设施（`support::spawn_recording_server` +
//! `support::build_ctx_with` + `agent_runtime::run_turn`）拿真实 wire 请求体，
//! 不退到 Ingredients 层——`spawn_recording_server` 是这个 crate 已有的、专门给
//! 独立测试捕获发出去的原始请求体字节用的设施（`transient_source_chain.rs` /
//! `send_plan_wiring_undo_restores_bytes.rs` 等已经在用），它跑得通、离
//! 「真正发给 provider 的字节」最近，没有理由绕道自己拼 `Ingredients`。

use std::sync::Arc;

use agent_core::{AgentId, Session, ToolSpec};
use agent_runtime::{run_session_start, run_turn, CallTiming, TimedRun, ToolTable};
use serde_json::json;

use crate::support;

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 总是成功、回一段固定文本的执行体。
fn ok_text(text: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable, _input: &serde_json::Value| -> Result<Arc<str>, Arc<str>> {
            Ok(Arc::from(text))
        },
    )
}

/// 断言：`run_session_start` 产出的 init 块文本，出现在两轮请求体的 system 段里，
/// 且**恰好一次**——不是零次（没进 prompt）也不是两次以上（前缀不稳定，
/// 红线 11 的经典症状：功能一切正常，只是每一轮都全价）。
#[test]
fn init_chunk_text_reaches_the_wire_system_segment_exactly_once_per_round() {
    const MARKER: &str = "SESSION-START-MARKER-af31c9";

    let dir = support::temp_dir("session-start-indep-into-prompt");
    let tools = ToolTable::builtin().with_timed(
        spec("alpha", "把标记文本塞进开局前缀"),
        CallTiming::SessionStart,
        ok_text(MARKER),
    );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("唯一的工具该成功");

    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_text("第一轮回复"),
        support::sse_text("第二轮回复"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    run_turn(&mut session, &mut ctx, "第一句话").expect("第一轮不该是 source failure");
    session.begin_turn();
    run_turn(&mut session, &mut ctx, "第二句话").expect("第二轮不该是 source failure");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "两轮各录到一条请求体");

    for (round, body) in bodies.iter().enumerate() {
        let system_text = wire_system_text(body);
        let occurrences = system_text.matches(MARKER).count();
        assert_eq!(
            occurrences, 1,
            "第 {round} 轮：init 块的文本该恰好出现一次在 system 段，实际 {occurrences} 次。\
             system 段：{system_text}"
        );
    }
}

/// 请求体里那条 `role: "system"` 消息的正文——模型真正看到的那串字符。
/// 照 `host_skills_index_is_byte_deterministic.rs::wire_system_text` 的同款手法，
/// 只是这里从 `spawn_recording_server` 录到的原始字符串取，不是从
/// `agent_providers::Encoded::body` 取。
fn wire_system_text(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("请求体该是合法 JSON");
    let messages = value["messages"].as_array().expect("请求体里该有 messages");
    messages
        .iter()
        .find(|m| m["role"] == "system")
        .expect("该有一条 system 消息")["content"]
        .as_str()
        .expect("system 消息该有文本正文")
        .to_string()
}
