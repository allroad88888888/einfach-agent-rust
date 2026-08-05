//! Pull polling reads the same retained event stream as browser SSE.

mod http_indep_support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::raw_http::request;
use http_indep_support::server_harness::{HarnessConfig, start};
use http_indep_support::sse_client::{SseClient, SseFrame};

fn poll(addr: SocketAddr, id: &str, last: Option<u64>, wait: Option<u64>) -> serde_json::Value {
    let path = format!("/sessions/{id}/events/poll");
    let last = last.map(|value| value.to_string());
    let wait = wait.map(|value| value.to_string());
    let mut headers = Vec::new();
    if let Some(value) = &last {
        headers.push(("Last-Event-ID", value.as_str()));
    }
    if let Some(value) = &wait {
        headers.push(("X-Poll-Wait-Ms", value.as_str()));
    }
    let response = request(addr, "GET", &path, &headers, None);
    assert_eq!(
        response.status,
        200,
        "poll should succeed: {}",
        response.body_str()
    );
    response.json()
}

fn drain_until_terminal(sse: &mut SseClient) -> Vec<SseFrame> {
    let mut frames = Vec::new();
    loop {
        let frame = sse
            .next_frame(Duration::from_secs(3))
            .unwrap_or_else(|| panic!("timed out after {frames:?}"));
        let done = frame.data.contains("TurnStatusChanged") && frame.data.contains("Done");
        frames.push(frame);
        if done {
            return frames;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_replays_the_same_frames_as_sse_without_duplicates() {
    let upstream = FakeUpstream::start(vec![Script::Text("hello from poll".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();
    let mut sse = SseClient::connect(server.addr, &id, None);

    assert_eq!(server.post_input(&id, "hi").status, 202);
    let streamed = drain_until_terminal(&mut sse);

    let replay = poll(server.addr, &id, None, None);
    let replayed_ids: Vec<_> = replay["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| frame["id"].as_u64().unwrap())
        .collect();
    let replayed_events: Vec<_> = replay["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| frame["event"].clone())
        .collect();
    let streamed_ids: Vec<_> = streamed.iter().map(|frame| frame.id.unwrap()).collect();
    let streamed_events: Vec<serde_json::Value> = streamed
        .iter()
        .map(|frame| serde_json::from_str(&frame.data).expect("SSE event should be JSON"))
        .collect();
    assert_eq!(replayed_ids, streamed_ids);
    assert_eq!(replayed_events, streamed_events);

    // `Last-Event-ID` keeps its SSE meaning: it is the last frame consumed, not
    // the next frame to read.  Reusing `next` therefore cannot skip a frame.
    let next = replay["next"].as_u64().unwrap();
    assert_eq!(next, *streamed_ids.last().unwrap());
    let caught_up = poll(server.addr, &id, Some(next), None);
    assert!(caught_up["frames"].as_array().unwrap().is_empty());
    assert_eq!(caught_up["next"].as_u64(), Some(next));
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_synthesizes_the_same_gap_envelope_as_sse() {
    let upstream = FakeUpstream::start(vec![
        Script::Text("one".to_string()),
        Script::Text("two".to_string()),
    ]);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            ring_capacity: 2,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();
    let mut sse = SseClient::connect(server.addr, &id, None);

    assert_eq!(server.post_input(&id, "first").status, 202);
    drain_until_terminal(&mut sse);
    assert_eq!(server.post_input(&id, "second").status, 202);
    drain_until_terminal(&mut sse);

    let gapped = poll(server.addr, &id, Some(0), None);
    let frames = gapped["frames"].as_array().unwrap();
    assert!(
        frames.len() > 1,
        "a gap must retain the buffered tail: {gapped}"
    );
    assert_eq!(frames[0]["event"]["agent"], "root");
    assert_eq!(frames[0]["event"]["event"]["type"], "gap");
    let gap_id = frames[0]["id"].as_u64().unwrap();

    // The gap id itself is the portable cursor: it replays the retained tail and
    // never synthesizes a second gap.
    let resumed = poll(server.addr, &id, Some(gap_id), None);
    let resumed_frames = resumed["frames"].as_array().unwrap();
    assert!(!resumed_frames.is_empty());
    assert_ne!(resumed_frames[0]["event"]["event"]["type"], "gap");
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_long_wait_returns_when_a_new_event_arrives() {
    let upstream = FakeUpstream::start(vec![Script::Text("wake the poll".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();

    // Build the hub before starting the turn, so the only response the long poll
    // can receive is the event produced after it begins waiting.
    assert!(
        poll(server.addr, &id, None, None)["frames"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let addr = server.addr;
    let poll_id = id.clone();
    let started = Instant::now();
    let pending = tokio::task::spawn_blocking(move || poll(addr, &poll_id, None, Some(1_000)));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(server.post_input(&id, "hi").status, 202);

    let result = pending.await.expect("poll task should not panic");
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "poll should wake before its timeout"
    );
    assert!(!result["frames"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completed_poll_starts_the_shared_grace_cancellation() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(150);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            cancel_grace: grace,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    // The request creates then releases the sole subscriber.  A following hanging
    // turn must use the very same grace path as an SSE disconnect.
    assert!(
        poll(server.addr, &id, None, None)["frames"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(server.post_input(&id, "hang").status, 202);
    tokio::time::sleep(grace * 4).await;

    let after_grace = poll(server.addr, &id, None, None);
    assert!(
        after_grace.to_string().contains("Cancelled"),
        "poll release should cancel the hanging turn: {after_grace}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_poll_keeps_a_hanging_turn_alive() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(150);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            cancel_grace: grace,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    assert!(
        poll(server.addr, &id, None, None)["frames"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(server.post_input(&id, "hang").status, 202);
    let first = poll(server.addr, &id, None, Some(500));
    let cursor = first["next"]
        .as_u64()
        .expect("the hanging turn should publish Thinking");

    let addr = server.addr;
    let poll_id = id.clone();
    let waiting =
        tokio::task::spawn_blocking(move || poll(addr, &poll_id, Some(cursor), Some(1_000)));
    tokio::time::sleep(grace * 3).await;
    assert!(
        !waiting.is_finished(),
        "a live long-poll must hold the shared SubscriberGuard"
    );

    assert_eq!(server.post_cancel(&id).status, 202);
    let result = waiting.await.expect("waiting poll should not panic");
    assert!(
        result.to_string().contains("Cancelled"),
        "explicit cancellation should wake the poll: {result}"
    );
}

/// Pull and SSE share one subscriber count, so neither transport can evict the
/// other: a gateway that polls once and leaves must not cancel a turn a browser
/// is still watching, and the countdown must still fire once the last of them
/// goes.  This is the whole payoff of reusing `SubscriberGuard` instead of
/// giving polling its own liveness bookkeeping — without this test the shared
/// counter could regress into two independent ones and every other test here
/// would stay green.
#[tokio::test(flavor = "multi_thread")]
async fn polling_and_sse_share_one_subscriber_count() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(150);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            cancel_grace: grace,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    // A browser watches the whole time; the gateway polls once and leaves.
    let sse = SseClient::connect(server.addr, &id, None);
    assert_eq!(server.post_input(&id, "hang").status, 202);

    // The poll's guard drops, but the count does not reach zero, so no countdown
    // may start.
    let _ = poll(server.addr, &id, None, None);
    tokio::time::sleep(grace * 4).await;
    let still_running = poll(server.addr, &id, None, None);
    assert!(
        !still_running.to_string().contains("Cancelled"),
        "a departing poll must not cancel a turn the SSE client is still watching: {still_running}"
    );

    // Now the last subscriber leaves and the same grace path must fire.
    drop(sse);
    tokio::time::sleep(grace * 4).await;
    let after_last_left = poll(server.addr, &id, None, None);
    assert!(
        after_last_left.to_string().contains("Cancelled"),
        "the countdown must still fire once both transports are gone: {after_last_left}"
    );
}

/// 057 验收「宽限内再拉不取消」在拉取这一层的直接证据。此前只有两处间接覆盖：
/// `hub/guard.rs` 的单测（机制层）和 `http_indep_grace_cancel.rs`（SSE 层），没有
/// 一条真的走过「poll → 宽限内再 poll → 断言轮次还活着」。
///
/// 形状是「网关按比宽限更快的节奏短轮询」而不是「poll 一次就长轮询挂住」，因为后者
/// 钉不住 `attach` 里那句 `task.abort()`：长轮询挂着的时候计数是 1，旧倒计时到点撞上
/// `Drop` 那条「二次确认 `== 0`」的第二道防线，abort 没了也照样不取消。短轮询把这层
/// 遮蔽掀开——旧倒计时是在两次 poll 的**间隙**到点的，那一刻计数确实是 0，二次确认
/// 拦不住，只有 abort 能。
#[tokio::test(flavor = "multi_thread")]
async fn re_polling_within_the_grace_period_aborts_the_countdown() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(300);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            cancel_grace: grace,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    // 这次 poll 建 hub、attach 又 drop：第一轮倒计时从这里开始跑，`grace` 之后到点。
    assert!(
        poll(server.addr, &id, None, None)["frames"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(server.post_input(&id, "hang").status, 202);

    // 每 0.4 个宽限回来一次，共 2.4 个宽限——每次 attach 都该 abort 掉上一次 drop
    // 起的倒计时，于是没有任何一轮倒计时活到到点。轮询节奏刻意不是宽限的整数分之一，
    // 让「没被 abort 的倒计时」的到点时刻落在两次 poll 的正中间。
    for _ in 0..6 {
        tokio::time::sleep(grace * 2 / 5).await;
        let batch = poll(server.addr, &id, None, None);
        assert!(
            !batch.to_string().contains("Cancelled"),
            "宽限内又来拉了，倒计时该被 abort：{batch}"
        );
    }
}

/// `poll_long_wait_returns_when_a_new_event_arrives` 走的是 `timeout` 的成功分支；
/// 这条走它的超时分支：等待窗口里始终没有帧，约 `wait` 之后返回空批，游标原地不动。
#[tokio::test(flavor = "multi_thread")]
async fn a_long_poll_with_no_traffic_returns_an_empty_batch_at_its_deadline() {
    let upstream = FakeUpstream::start(vec![Script::Text("never requested".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();

    // 从不 `POST /input`：这次等待只可能靠自己的 deadline 结束。
    let wait = Duration::from_millis(400);
    let started = Instant::now();
    let idle = poll(server.addr, &id, Some(7), Some(wait.as_millis() as u64));
    let elapsed = started.elapsed();

    assert!(
        elapsed >= wait - Duration::from_millis(50),
        "该等满 wait 才返回，实际 {elapsed:?}"
    );
    assert!(
        elapsed < wait * 3,
        "到点就该返回，不该拖到读超时，实际 {elapsed:?}"
    );
    assert!(
        idle["frames"].as_array().unwrap().is_empty(),
        "等待期间一帧都没有：{idle}"
    );
    assert_eq!(idle["next"].as_u64(), Some(7), "空批不能推进游标：{idle}");
}

/// 解析不出数字的 `X-Poll-Wait-Ms` 静默降级成 `wait = 0`，跟垃圾 `Last-Event-ID`
/// 同款（降级，不是 400）。`routes/poll.rs` 的模块内单测只钉了解析函数，这条把它
/// 钉到线上：真发一个垃圾 header，仍是 200 且立刻返回。
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_poll_wait_header_degrades_to_an_immediate_poll() {
    let upstream = FakeUpstream::start(vec![Script::Text("never requested".to_string())]);
    let server = start(upstream.endpoint(), HarnessConfig::default()).await;
    let id = server.create_session();
    let path = format!("/sessions/{id}/events/poll");

    for garbage in ["not-a-number", "-1", "1.5", ""] {
        let started = Instant::now();
        let response = request(
            server.addr,
            "GET",
            &path,
            &[("X-Poll-Wait-Ms", garbage)],
            None,
        );
        assert_eq!(
            response.status,
            200,
            "{garbage:?} 不该让请求失败：{}",
            response.body_str()
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{garbage:?} 该立刻返回，实际 {:?}",
            started.elapsed()
        );
        assert!(
            response.json()["frames"].as_array().unwrap().is_empty(),
            "{garbage:?}：{}",
            response.body_str()
        );
    }
}
