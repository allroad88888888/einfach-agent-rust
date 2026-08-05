//! 031 独立测试 agent：双客户端（issue 031 验收「两条 SSE 连接同帧序；断一条
//! 另一条不受影响、会话不被宽限杀（引用计数 >0）」）。

mod http_indep_support;

use std::time::Duration;

use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::server_harness::{HarnessConfig, start};
use http_indep_support::sse_client::{SseClient, SseFrame};

fn collect_until_terminal(sse: &mut SseClient) -> Vec<SseFrame> {
    let mut frames = Vec::new();
    loop {
        let Some(frame) = sse.next_frame(Duration::from_secs(3)) else { panic!("等终态超时，已收到 {frames:?}") };
        let terminal = frame.data.contains("TurnStatusChanged") && frame.data.contains("Done");
        frames.push(frame);
        if terminal || frames.len() > 30 {
            break;
        }
    }
    frames
}

/// 两条独立 SSE 连接订阅同一个 session，看到完全相同的帧序（id 和内容逐帧
/// 相同）。
#[tokio::test(flavor = "multi_thread")]
async fn two_sse_clients_see_the_identical_frame_sequence() {
    let upstream = FakeUpstream::start(vec![Script::Text("shared reply".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();

    let mut client_a = SseClient::connect(server.addr, &id, None);
    let mut client_b = SseClient::connect(server.addr, &id, None);

    let resp = server.post_input(&id, "hi");
    assert_eq!(resp.status, 202);

    let frames_a = collect_until_terminal(&mut client_a);
    let frames_b = collect_until_terminal(&mut client_b);

    assert_eq!(frames_a, frames_b, "两条连接该看到完全相同的帧序（id + 内容）");
    assert!(!frames_a.is_empty());
}

/// 两条连接都在场时，断开其中一条：另一条继续正常收帧不受影响；断开的那条
/// 不会把会话拖进宽限取消——引用计数（还有一个订阅者在）该保护这一点，宽限
/// 计时器压根不该启动。
#[tokio::test(flavor = "multi_thread")]
async fn disconnecting_one_client_does_not_affect_the_other_or_trigger_grace_cancel() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(200);
    let server = start(upstream.endpoint(), HarnessConfig { cancel_grace: grace, ..HarnessConfig::default() }).await;
    let id = server.create_session();

    let mut client_a = SseClient::connect(server.addr, &id, None);
    let client_b = SseClient::connect(server.addr, &id, None);

    let resp = server.post_input(&id, "hi");
    assert_eq!(resp.status, 202);

    // 两条都确认收到了 Thinking，证明轮真的在飞、两条连接都真的挂上了。
    let thinking_a = client_a.next_frame(Duration::from_secs(2)).expect("A 该收到 Thinking");
    assert!(thinking_a.data.contains("Thinking"));

    // 断开 B，只留 A。
    drop(client_b);

    // 安静等一段远超宽限期的时长——只有一条订阅者断开，另一条还在，引用计数
    // 该保护会话不进宽限倒计时。
    tokio::time::sleep(grace * 6).await;

    // A 不该看到 Cancelled（会话没被宽限杀）。
    let after_wait = client_a.next_frame(Duration::from_millis(300));
    if let Some(frame) = &after_wait {
        assert!(!frame.data.contains("Cancelled"), "还有 A 在订阅，不该被宽限取消：{frame:?}");
    }
    let status = server.get_status(&id);
    assert_eq!(status.json()["status"], "alive");

    // 收尾：主动 cancel，确认 A 这条连接依然完好、命令依然能送达并生效。
    let cancel_resp = server.post_cancel(&id);
    assert_eq!(cancel_resp.status, 202);
    let mut frames: Vec<_> = after_wait.into_iter().collect();
    loop {
        let Some(frame) = client_a.next_frame(Duration::from_secs(3)) else { panic!("A 该还能正常收帧，已收到 {frames:?}") };
        let cancelled = frame.data.contains("Cancelled");
        frames.push(frame);
        if cancelled || frames.len() > 20 {
            break;
        }
    }
    assert!(frames.iter().any(|f| f.data.contains("Cancelled")), "主动 cancel 该照常生效：{frames:?}");
}
