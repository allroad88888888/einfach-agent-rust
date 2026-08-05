//! 031 独立测试 agent：先 input 后首连（本 issue 独测任务书原文第 3 条——
//! 「POST 两轮 input 完成后才第一次连 SSE → 能从缓冲收到全部历史帧（hub eager
//! 创建的行为面）」）。
//!
//! # 分歧（如实记录，见下面两个测试的对照）
//!
//! 实测：**完全不带 `Last-Event-ID` 请求头**的首次连接（真实浏览器第一次开
//! `EventSource` 时的样子——从来没连过，自然没有可带的 id）不会收到任何历史
//! 帧，直接接上「此刻之后」的直播，此前两轮 input 产生的全部事件对这个连接
//! 永久不可见。换成**显式带 `Last-Event-ID: 0`** 重连，同样的两轮历史立刻从
//! id 1 开始完整补发——补发引擎本身是对的，缺的是「请求头完全不存在」这条
//! 路径没有接上它。`reconnect_without_any_last_event_id_header_should_still_see_buffered_history`
//! 精确复现任务书原文场景（031 分歧修复后已转正，缺头=从最旧可用帧补起）
//! `cargo test -p agent-server` 全绿；`reconnect_with_explicit_last_event_id_zero_sees_full_history`
//! 钉住「机制本身没坏」这一半，帮助后续排障定位到具体缺的是哪一条路径。

mod http_indep_support;

use std::time::Duration;

use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::server_harness::{HarnessConfig, start};
use http_indep_support::sse_client::SseClient;

async fn two_rounds_before_any_sse_connect() -> (
    http_indep_support::server_harness::TestServer,
    String,
    FakeUpstream,
) {
    let upstream = FakeUpstream::start(vec![
        Script::Text("first reply".to_string()),
        Script::Text("second reply".to_string()),
    ]);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            ring_capacity: 256,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    let r1 = server.post_input(&id, "hello");
    assert_eq!(r1.status, 202, "input 该是 202，body={}", r1.body_str());
    tokio::time::sleep(Duration::from_millis(800)).await;

    let r2 = server.post_input(&id, "again");
    assert_eq!(r2.status, 202);
    tokio::time::sleep(Duration::from_millis(800)).await;

    assert_eq!(
        server.get_status(&id).json()["status"],
        "alive",
        "两轮结束后 session 该还活着"
    );
    (server, id, upstream)
}

fn collect_two_terminals(sse: &mut SseClient) -> Vec<http_indep_support::sse_client::SseFrame> {
    let mut frames = Vec::new();
    let mut terminal_count = 0;
    loop {
        let Some(frame) = sse.next_frame(Duration::from_secs(3)) else {
            panic!("等历史帧超时，已收到 {} 帧：{frames:?}", frames.len())
        };
        if frame.data.contains("TurnStatusChanged") && frame.data.contains("Done") {
            terminal_count += 1;
        }
        frames.push(frame);
        if terminal_count >= 2 || frames.len() > 40 {
            break;
        }
    }
    frames
}

/// 任务书原文场景，逐字对应：不带 `Last-Event-ID`（真第一次连接的样子）连上
/// 之后该能收到两轮的全部历史帧。**目前会失败**——见文件头分歧说明。
#[tokio::test(flavor = "multi_thread")]
async fn reconnect_without_any_last_event_id_header_should_still_see_buffered_history() {
    let (server, id, _upstream) = two_rounds_before_any_sse_connect().await;

    let mut sse = SseClient::connect(server.addr, &id, None);
    assert_eq!(sse.status(), 200);

    let frames = collect_two_terminals(&mut sse);

    assert_eq!(
        frames[0].id,
        Some(1),
        "全新客户端不带 Last-Event-ID，该从缓冲最早一帧（id 1）开始收"
    );
    let ids: Vec<u64> = frames.iter().map(|f| f.id.unwrap()).collect();
    for w in ids.windows(2) {
        assert_eq!(w[1], w[0] + 1, "id 该连续无缺口：{ids:?}");
    }
    let joined: String = frames.iter().map(|f| f.data.as_str()).collect();
    assert!(
        joined.contains("first reply"),
        "第一轮的回复该在缓冲里：{joined}"
    );
    assert!(
        joined.contains("second reply"),
        "第二轮的回复该在缓冲里：{joined}"
    );
}

/// 同样两轮历史，换成显式 `Last-Event-ID: 0` 重连——证明补发引擎本身没坏，
/// 只是「请求头完全不存在」那条入口没有接上它（分歧说明见文件头）。
#[tokio::test(flavor = "multi_thread")]
async fn reconnect_with_explicit_last_event_id_zero_sees_full_history() {
    let (server, id, _upstream) = two_rounds_before_any_sse_connect().await;

    let mut sse = SseClient::connect(server.addr, &id, Some(0));
    assert_eq!(sse.status(), 200);

    let frames = collect_two_terminals(&mut sse);

    assert_eq!(frames[0].id, Some(1));
    let joined: String = frames.iter().map(|f| f.data.as_str()).collect();
    assert!(joined.contains("first reply"));
    assert!(joined.contains("second reply"));
}
