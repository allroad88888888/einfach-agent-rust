//! 验收清单第三条：轮进行中发 `Cancel` → 数百 ms 内本轮 `Failed(Cancelled)`
//! ——复用 `agent-runtime/tests/cancel.rs` 同款轮询手法（假服务器只发响应头
//! 就挂住，后台任务在 200ms 后翻取消标志，断言收尾发生在置位之后的几个 poll
//! 间隔内，而不是撞上 5s 的 provider 超时预算）。
//!
//! 顺带钉住 027 的自动擦除策略在 actor 里一样生效：取消落地成
//! `Failed(Cancelled)` 之后，紧跟着一条 `SessionEvent::Undo(Applied)`。

mod support;

use std::time::{Duration, Instant};

use agent_core::{Failure, TurnStatus};
use agent_server::{Command, SessionEvent, UndoOutcome};

use support::server::{FakeServer, Script};

#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_an_in_flight_turn_lands_failed_cancelled_within_hundreds_of_ms() {
    let server = FakeServer::start(vec![Script::HangAfterHeaders]);
    let registry = agent_server::SessionRegistry::new();
    let handle = registry.open(support::open_spec("cancel-me", server.endpoint(), None)).unwrap();

    let mut sub = handle.subscribe();
    handle.send(Command::Input("say something slow".to_string())).unwrap();

    let cancel_handle = handle.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_handle.cancel();
    });

    let start = Instant::now();
    let events = support::collect_until_terminal(&mut sub, Duration::from_secs(3)).await;
    let elapsed = start.elapsed();

    assert_eq!(support::terminal_status(&events), Some(TurnStatus::Failed(Failure::Cancelled)));
    assert!(
        elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(2),
        "该在置位之后的几个 poll 间隔内收尾，不该等到 5s 的超时预算，实际 {elapsed:?}"
    );

    // 027 的自动擦除策略：取消轮结束后紧跟一次 undo_turn，结果是 Applied
    // （这一轮除了一条 user_input 之外什么都没落地，没有屏障可挡）。
    let follow_up = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("该收到取消轮自动擦除的 Undo 事件")
        .expect("事件流不该在这里结束");
    assert_eq!(follow_up.agent.as_str(), "root", "自动擦除是会话级动作，该标 root");
    assert!(matches!(follow_up.event, SessionEvent::Undo(UndoOutcome::Applied { .. })), "{follow_up:?}");
}
