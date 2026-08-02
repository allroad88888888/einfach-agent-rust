//! 031 独立测试 agent：帧格式契约（issue 031 验收「SSE 收到的每帧 data 是
//! `{"type":"...","data":...}` 邻接标签；`text_delta` 的 data 是纯字符串；帧
//! id 单调；两个 header 逐字节在响应头里」）。034 起：SSE 帧 data 外面多了一层
//! `Frame` 信封（`{"agent":"...","event":{"type":"...","data":...}}`），这个
//! 文件顺带升级验证的就是这一层——`event` 字段内部仍然是原来那套邻接标签
//! 契约，只是要先从信封里把它取出来。自己拼的 HTTP 客户端见
//! `http_indep_support/`，不碰实现方的 `http/` 源码或 `tests/http_*.rs`。

mod http_indep_support;

use std::time::Duration;

use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::server_harness::{HarnessConfig, start};
use http_indep_support::sse_client::SseClient;

/// 两个必发 header（`Cache-Control: no-cache`、`X-Accel-Buffering: no`）要出现
/// 在每一次 `GET /events` 的响应头里，值逐字节匹配 issue 原文——header 名字大小写
/// 不敏感（HTTP 语义本身如此，hyper 在线上把它们规整成小写属于合规行为，不是
/// 服务端「没发对」）。
#[tokio::test(flavor = "multi_thread")]
async fn sse_response_always_carries_the_two_required_headers() {
    let upstream = FakeUpstream::start(vec![Script::Text("hi".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();

    let sse = SseClient::connect(server.addr, &id, None);
    assert_eq!(sse.status(), 200);
    assert_eq!(sse.header("cache-control"), Some("no-cache"), "raw head:\n{}", sse.head.raw);
    assert_eq!(sse.header("x-accel-buffering"), Some("no"), "raw head:\n{}", sse.head.raw);
}

/// `text_delta` 的 `data` 字段必须是纯 JSON 字符串（不是 `{"text": "hi"}` 这种
/// 对象）——邻接标签对 newtype 变体的落地方式。同时钉住帧 id 从 1 开始严格单调。
#[tokio::test(flavor = "multi_thread")]
async fn text_delta_data_is_a_plain_string_and_frame_ids_are_monotonic() {
    let upstream = FakeUpstream::start(vec![Script::Text("hello world".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();

    let mut sse = SseClient::connect(server.addr, &id, None);
    let resp = server.post_input(&id, "hi");
    assert_eq!(resp.status, 202, "input 该是 fire-and-forget 的 202，body={}", resp.body_str());

    let mut frames = Vec::new();
    while let Some(frame) = sse.next_frame(Duration::from_secs(3)) {
        let is_terminal = frame.data.contains("\"turn_status_changed\"") || frame.data.contains("TurnStatusChanged");
        frames.push(frame);
        if is_terminal && frames.last().unwrap().data.contains("Done") {
            break;
        }
        if frames.len() > 20 {
            break;
        }
    }

    assert!(!frames.is_empty(), "至少要收到一帧");

    // 帧 id 严格单调递增，从 1 开始。
    let ids: Vec<u64> = frames.iter().map(|f| f.id.expect("每一帧都该带 id")).collect();
    assert_eq!(ids[0], 1, "首帧 id 该是 1，实际 {ids:?}");
    for w in ids.windows(2) {
        assert!(w[1] > w[0], "帧 id 必须严格单调递增：{ids:?}");
    }

    // 034：帧 data 最外层是 `Frame` 信封——`agent` 字段先于 `event`（字段声明
    // 顺序，serde 默认按结构体字段顺序序列化），root 会话唯一的 agent 就是
    // "root"。信封本身也是逐字节钉的一部分，不是只挑 `event` 那一半看。
    let text_delta = frames.iter().find(|f| f.data.starts_with("{\"agent\":\"root\",\"event\":{\"type\":\"text_delta\"")).unwrap_or_else(|| {
        panic!("没找到 text_delta 帧，收到的帧：{frames:?}")
    });
    let value: serde_json::Value = serde_json::from_str(&text_delta.data).unwrap();
    assert_eq!(value["agent"], "root");
    assert_eq!(value["event"]["type"], "text_delta");
    assert!(value["event"]["data"].is_string(), "text_delta 的 data 必须是纯字符串，实际：{value}");
    assert_eq!(value["event"]["data"].as_str().unwrap(), "hello world");

    // 邻接标签对任意变体形状都成立：随手挑一个对象形态的变体（turn_guard）验证
    // 它的 data 是 JSON 对象而不是字符串——两种形状都要能正确落地，不是巧合。
    if let Some(guard) = frames.iter().find(|f| f.data.contains("\"type\":\"turn_guard\"")) {
        let value: serde_json::Value = serde_json::from_str(&guard.data).unwrap();
        assert_eq!(value["agent"], "root");
        assert!(value["event"]["data"].is_object(), "turn_guard 的 data 该是对象：{value}");
    }
}
