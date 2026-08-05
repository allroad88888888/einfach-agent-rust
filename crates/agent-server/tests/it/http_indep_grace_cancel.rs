//! 031 独立测试 agent：宽限取消（issue 031 验收「断开所有订阅 → 5s 后在飞轮
//! 被取消（假上游挂住不回，断言取消而非等到超时）」+ 实做记录「刷新页面不该
//! 杀轮次」的宽限期）。反例：宽限期内重连，轮继续跑不被杀。

use std::time::Duration;

use crate::http_indep_support::fake_upstream::{FakeUpstream, Script};
use crate::http_indep_support::server_harness::{HarnessConfig, start};
use crate::http_indep_support::sse_client::SseClient;

fn is_cancelled_terminal(data: &str) -> bool {
    data.contains("TurnStatusChanged") && data.contains("Cancelled")
}

fn is_done_terminal(data: &str) -> bool {
    data.contains("TurnStatusChanged") && data.contains("Done")
}

/// 断开唯一的 SSE 订阅者、安静等待一段远超宽限期的时长（**只重连一次**——
/// 反复轮询重连会不断触发「重连打断计时」逻辑，等于用探测行为本身持续
/// 干扰被测的倒计时机制，这是 031 实做记录里记录过的一个坑）、重连后应该
/// 看到 `Failed(Cancelled)` 语义的终态帧。
#[tokio::test(flavor = "multi_thread")]
async fn disconnecting_all_subscribers_cancels_the_in_flight_turn_after_grace() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(200);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            cancel_grace: grace,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    let mut sse = SseClient::connect(server.addr, &id, None);
    let resp = server.post_input(&id, "hi");
    assert_eq!(resp.status, 202);

    // 确认轮真的在飞（先看到 Thinking 通报），再断开。
    let thinking = sse
        .next_frame(Duration::from_secs(2))
        .expect("该先看到 Thinking 通报");
    assert!(
        thinking.data.contains("Thinking"),
        "第一帧该是 Thinking：{}",
        thinking.data
    );
    drop(sse);

    // 安静等一段远超宽限期的时长，只重连这一次。
    tokio::time::sleep(grace * 6).await;

    let mut reconnected = SseClient::connect(server.addr, &id, Some(0));
    let mut frames = Vec::new();
    let outcome = loop {
        let Some(frame) = reconnected.next_frame(Duration::from_secs(3)) else {
            panic!("等取消终态超时，已收到：{frames:?}")
        };
        let cancelled = is_cancelled_terminal(&frame.data);
        let done = is_done_terminal(&frame.data);
        frames.push(frame);
        if cancelled || done {
            break (cancelled, done);
        }
        if frames.len() > 20 {
            panic!("帧数超限还没等到终态：{frames:?}");
        }
    };
    assert_eq!(
        outcome,
        (true, false),
        "断开宽限期后该看到 Cancelled 终态，不是 Done：{frames:?}"
    );

    let status = server.get_status(&id);
    assert_eq!(
        status.json()["status"],
        "alive",
        "取消是轮失败，不是 session 死——session 该还活着"
    );
}

/// 反例：宽限期内重连，轮不该被杀——继续跑，最终能正常收到（我们主动发出
/// 的）取消或完成，而不是「宽限期一到就杀」那种不管有没有人重连都杀的粗暴
/// 实现。这里用「宽限期内重连后，再等一段超过原宽限期总时长的时间，仍然没
/// 看到 Cancelled」来判定「没被杀」，然后主动 `POST /cancel` 干净收尾。
#[tokio::test(flavor = "multi_thread")]
async fn reconnecting_within_grace_keeps_the_turn_alive() {
    let upstream = FakeUpstream::start(vec![Script::Hang]);
    let grace = Duration::from_millis(400);
    let server = start(
        upstream.endpoint(),
        HarnessConfig {
            cancel_grace: grace,
            ..HarnessConfig::default()
        },
    )
    .await;
    let id = server.create_session();

    let mut sse = SseClient::connect(server.addr, &id, None);
    let resp = server.post_input(&id, "hi");
    assert_eq!(resp.status, 202);
    let thinking = sse
        .next_frame(Duration::from_secs(2))
        .expect("该先看到 Thinking 通报");
    assert!(thinking.data.contains("Thinking"));
    drop(sse);

    // 宽限期内重连（远早于宽限期到期）。
    tokio::time::sleep(grace / 4).await;
    let mut reconnected = SseClient::connect(server.addr, &id, Some(0));

    // 再等一段比原宽限期还长的时间——如果重连没有打断计时器，这时候该已经
    // 被杀了；如果打断了，轮还在飞，这段时间里不该出现 Cancelled。
    let quiet = reconnected.next_frame(grace * 3);
    if let Some(frame) = &quiet {
        assert!(
            !is_cancelled_terminal(&frame.data),
            "宽限期内重连之后不该被取消：{frame:?}"
        );
    }

    let status = server.get_status(&id);
    assert_eq!(status.json()["status"], "alive");

    // 主动收尾：发 Cancel，确认命令本身仍然生效（顺便验证「没被自动杀」不是
    // 因为 session 已经失联）。
    let cancel_resp = server.post_cancel(&id);
    assert_eq!(cancel_resp.status, 202);
    let mut frames = vec![quiet].into_iter().flatten().collect::<Vec<_>>();
    loop {
        let Some(frame) = reconnected.next_frame(Duration::from_secs(3)) else {
            panic!("主动 cancel 之后等终态超时，已收到：{frames:?}")
        };
        let cancelled = is_cancelled_terminal(&frame.data);
        frames.push(frame);
        if cancelled || frames.len() > 20 {
            break;
        }
    }
    assert!(
        frames.iter().any(|f| is_cancelled_terminal(&f.data)),
        "主动 cancel 该照常生效：{frames:?}"
    );
}
