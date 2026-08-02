//! 031 独立测试 agent：undo/redo 语义过 HTTP（issue 031 验收「两轮后 POST
//! undo(turn) → SSE 出 undo outcome 帧 → 下一轮 input 的上游请求体不含被退
//! 内容（假上游存请求体断言——027 的证法搬到 HTTP 层）」）。

mod http_indep_support;

use std::time::Duration;

use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::server_harness::{HarnessConfig, start};
use http_indep_support::sse_client::SseClient;

fn drain_until_terminal(sse: &mut SseClient) {
    loop {
        let Some(frame) = sse.next_frame(Duration::from_secs(3)) else { panic!("等终态超时") };
        if frame.data.contains("TurnStatusChanged") && (frame.data.contains("Done") || frame.data.contains("Cancelled") || frame.data.contains("Failed")) {
            return;
        }
    }
}

fn next_matching(sse: &mut SseClient, needle: &str, budget: Duration) -> String {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            panic!("等 {needle} 超时");
        }
        let Some(frame) = sse.next_frame(remaining) else { panic!("连接断了，还没等到 {needle}") };
        if frame.data.contains(needle) {
            return frame.data;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_turn_outcome_frame_and_upstream_body_no_longer_carries_the_undone_turn() {
    let upstream = FakeUpstream::start(vec![
        Script::Text("first reply".to_string()),
        Script::Text("second reply".to_string()),
        Script::Text("third reply".to_string()),
    ]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();

    let mut sse = SseClient::connect(server.addr, &id, None);

    server.post_input(&id, "distinctive-marker-round-one");
    drain_until_terminal(&mut sse);
    server.post_input(&id, "distinctive-marker-round-two");
    drain_until_terminal(&mut sse);

    assert_eq!(upstream.request_count(), 2);
    let round_two_body = upstream.bodies()[1].clone();
    assert!(round_two_body.contains("distinctive-marker-round-one"), "第二轮的请求体该带着第一轮的历史（还没退之前）");
    assert!(round_two_body.contains("distinctive-marker-round-two"));

    // 退第二轮（turn 粒度，不 force）。
    let undo_resp = server.post_undo(&id, "turn", false);
    assert_eq!(undo_resp.status, 202, "body={}", undo_resp.body_str());

    let undo_frame = next_matching(&mut sse, "\"type\":\"undo\"", Duration::from_secs(3));
    let undo_json: serde_json::Value = serde_json::from_str(&undo_frame).unwrap();
    // 034：帧最外层是 `Frame` 信封（`agent`/`event`），`SessionEvent` 的
    // 邻接标签（"type"/"data"）在 `event` 字段里面。
    assert_eq!(undo_json["agent"], "root");
    assert_eq!(undo_json["event"]["type"], "undo");
    // UndoOutcome::Applied { entries, turn_id } 邻接标签之下再套一层
    // UndoOutcome 自己的 tag/content（"applied"/"blocked"/"nothing"）。
    assert_eq!(undo_json["event"]["data"]["type"], "applied", "两轮都没碰不可逆工具，第二轮该能干净退掉：{undo_json}");

    // 第三轮 input：上游请求体不该再带第二轮的内容（027 的证法搬到 HTTP 层）。
    server.post_input(&id, "distinctive-marker-round-three");
    drain_until_terminal(&mut sse);

    assert_eq!(upstream.request_count(), 3);
    let round_three_body = upstream.bodies()[2].clone();
    assert!(round_three_body.contains("distinctive-marker-round-one"), "第一轮没被退，该还在：{round_three_body}");
    assert!(!round_three_body.contains("distinctive-marker-round-two"), "第二轮被退了，不该再出现在上游请求体里：{round_three_body}");
    assert!(round_three_body.contains("distinctive-marker-round-three"));
}

#[tokio::test(flavor = "multi_thread")]
async fn redo_outcome_frame_restores_the_undone_turn_into_the_next_upstream_body() {
    let upstream = FakeUpstream::start(vec![Script::Text("first reply".to_string()), Script::Text("third reply".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();
    let mut sse = SseClient::connect(server.addr, &id, None);

    server.post_input(&id, "only-round");
    drain_until_terminal(&mut sse);

    let undo_resp = server.post_undo(&id, "turn", false);
    assert_eq!(undo_resp.status, 202);
    next_matching(&mut sse, "\"type\":\"undo\"", Duration::from_secs(3));

    let redo_resp = server.post_redo(&id);
    assert_eq!(redo_resp.status, 202, "body={}", redo_resp.body_str());
    let redo_frame = next_matching(&mut sse, "\"type\":\"redo\"", Duration::from_secs(3));
    let redo_json: serde_json::Value = serde_json::from_str(&redo_frame).unwrap();
    assert_eq!(redo_json["event"]["data"]["type"], "applied", "退了又还，该正常 applied：{redo_json}");

    // 下一轮 input：上游请求体该重新带上 redo 回来的那一轮。
    server.post_input(&id, "next-round");
    drain_until_terminal(&mut sse);
    let body = upstream.bodies().last().unwrap().clone();
    assert!(body.contains("only-round"), "redo 之后该恢复，body={body}");
}
