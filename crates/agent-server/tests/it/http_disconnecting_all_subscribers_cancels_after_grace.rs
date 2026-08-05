//! 验收清单第四条：断开所有订阅 → 宽限期过后在飞轮被取消（假上游挂住不回，
//! 断言取消而非等到超时）——外加 issue 原文点名的「宽限期内重连取消计时」。
//!
//! **时间预算刻意留得很宽**（等待窗口用秒级、`provider_timeout` 调到一分钟）：
//! 这条测试断的是「取消是宽限计时器动的手，不是等 provider 自然超时」，
//! 只要「看到 `Cancelled`」和「provider 自然超时（1 分钟）」之间留出足够大的
//! 数量级差距，具体宽限期是 200ms 还是被系统调度抖动拖到大几百毫秒并不重要
//! ——`cargo test --workspace` 并发跑几百个测试时的调度延迟比空闲环境大得多，
//! 之前吃过一次这个亏（紧绷的固定 sleep 在独立跑时稳定通过、全量并发跑时偶发
//! 超时失败）。

mod support;

use std::time::Duration;

use agent_core::{Failure, Notice, TurnStatus};
use agent_server::{Frame, ServerConfig, SessionEvent};

use support::http_client;
use support::server::{FakeServer, Script};

/// provider 永远不自然超时（一分钟，测试不会真的等到这里）——这样「看到
/// `Cancelled`」只可能来自宽限计时器，不可能是巧合撞上 provider 超时。
const PROVIDER_NEVER_TIMES_OUT: Duration = Duration::from_secs(60);
const GRACE: Duration = Duration::from_millis(200);
/// 断开之后安静等待的时长——是 `GRACE` 的 15 倍，给系统调度抖动留足余量，又
/// 远小于 `PROVIDER_NEVER_TIMES_OUT`，两头数量级都拉开即可，见本文件模块文档。
const WAIT_FOR_CANCELLED: Duration = Duration::from_secs(3);

fn config(c: ServerConfig) -> ServerConfig {
    c.with_ring_capacity(5).with_cancel_grace(GRACE).with_sse_keep_alive(Duration::from_millis(100))
}

async fn start_with_slow_provider_timeout(endpoint: String) -> support::http_server::TestServer {
    let mut template = support::http_server::session_template(endpoint);
    template.provider_timeout = Some(PROVIDER_NEVER_TIMES_OUT);
    support::http_server::start_at_with_template("127.0.0.1:0".parse().unwrap(), template, config).await
}

async fn create_session(addr: std::net::SocketAddr) -> String {
    let create = http_client::request(addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    support::extract_json_string_field(&create.body, "id")
}

/// 034：SSE 帧 data 是 `Frame` 信封（`{"agent":...,"event":{...}}`），不再是
/// 裸的 `SessionEvent`——解析进 `Frame` 再看 `.event`。
fn is_cancelled(data: &str) -> bool {
    matches!(
        serde_json::from_str::<Frame>(data),
        Ok(Frame { event: SessionEvent::Notice(Notice::TurnStatusChanged { status: TurnStatus::Failed(Failure::Cancelled) }), .. })
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnecting_the_only_subscriber_cancels_the_flying_turn_after_the_grace_period() {
    // 上游只发响应头,长时间不回数据——`run_turn` 会一直卡在等 provider 上。
    let upstream = FakeServer::start(vec![Script::HangAfterHeaders]);
    let server = start_with_slow_provider_timeout(upstream.endpoint()).await;
    let id = create_session(server.addr).await;

    let (_, _, sse) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);

    drop(sse); // 唯一的订阅者断开——宽限计时器该从这一刻开始倒数。

    // **只重连一次**：`SubscriberGuard::attach` 一有新连接就会 abort 掉正在
    // 倒数的宽限计时器（这就是「宽限期内重连取消计时」的实现，见 `crate::http::
    // hub::guard` 模块文档）——早先这里写成「反复重连轮询直到看到 Cancelled」，
    // 结果每一次探测性重连都把倒计时自己打断了一次，宽限计时器永远没机会跑完，
    // 是测试自己的观测行为在干扰被测的机制（经典的『测量本身改变了系统』）。
    // 正确做法是安安静静等一段足够长的时间（不接触 hub），再连**一次**去看
    // 结果——这段独占等待期间没有任何订阅者，宽限计时器不会被打断。
    tokio::time::sleep(WAIT_FOR_CANCELLED).await;

    // 这一次重连（带 `Last-Event-ID: 0` 拿到从头开始的完整补发）确认真的被
    // 取消了——顺带证明了 hub 在没有订阅者的这段时间里仍然继续录像
    // （ARCHITECTURE.md 的既有取消传播语义：不白烧 token）。
    let (status, _, mut reconnected) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), Some(0));
    assert_eq!(status, 200);
    let mut saw_cancelled = false;
    while let Some(frame) = reconnected.next_event(Duration::from_secs(3)) {
        if is_cancelled(&frame.data) {
            saw_cancelled = true;
            break;
        }
    }
    assert!(saw_cancelled, "断开所有订阅、过了宽限期，在飞的轮次该被取消（不是等 provider 自然超时）");
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnecting_within_the_grace_period_keeps_the_turn_alive() {
    let upstream = FakeServer::start(vec![Script::HangAfterHeaders]);
    let server = start_with_slow_provider_timeout(upstream.endpoint()).await;
    let id = create_session(server.addr).await;

    let (_, _, first) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);

    drop(first);
    tokio::time::sleep(Duration::from_millis(30)).await; // 宽限期是 200ms，这里远没到，重连该来得及打断倒计时。
    let (status, _, mut second) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status, 200, "宽限期内重连该成功接上（不是被拒绝或者一个已经死掉的 session）");

    // 等一段远超原本宽限期截止时间、又远小于 provider 超时的时长，确认没有被
    // 取消——重连已经打断了倒计时。
    let mut saw_cancelled = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let Some(frame) = second.next_event(Duration::from_millis(300)) else { continue };
        if is_cancelled(&frame.data) {
            saw_cancelled = true;
            break;
        }
    }
    assert!(!saw_cancelled, "宽限期内已经重连了，这一轮不该被取消");
}
