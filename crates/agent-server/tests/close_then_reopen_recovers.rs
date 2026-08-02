//! 验收清单第五条：`close` 后线程 join、持久化文件完整可 `open` 恢复。
//!
//! 证法：第一个 actor 跑完一轮（产出至少一条 `user_input` entry）、`close`；
//! 第二个 actor 用同一个落盘路径重新 `open`，**不发任何 `Input`**，直接发
//! `Undo`——如果历史是空的（没恢复上），结果只能是 `Nothing`；能看到
//! `Applied` 就证明上一个 actor 写的 entry 真的被读回来、重建进了新 `Session`。

mod support;

use std::time::Duration;

use agent_server::{Command, Granularity, SessionEvent, UndoOutcome};

use support::server::{FakeServer, Script};
use support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn a_session_closed_and_reopened_recovers_its_history() {
    let server = FakeServer::start(vec![Script::Immediate(text_reply("remembered"))]);
    let store_path = support::temp_dir("close-reopen").join("session.jsonl");

    let registry = agent_server::SessionRegistry::new();
    let handle = registry.open(support::open_spec("s", server.endpoint(), Some(store_path.clone()))).unwrap();

    let mut sub = handle.subscribe();
    handle.send(Command::Input("remember this".to_string())).unwrap();
    support::collect_until_terminal(&mut sub, Duration::from_secs(5)).await;

    registry.close(&agent_server::SessionId::from("s")).expect("优雅关闭该成功——actor 没崩过");

    // 重新 open：同一个落盘路径，全新的 registry 表项（原来那条已经被 close 摘掉）。
    let handle2 = registry.open(support::open_spec("s", server.endpoint(), Some(store_path))).unwrap();
    let mut sub2 = handle2.subscribe();
    handle2.send(Command::Undo { granularity: Granularity::Turn, force: false }).unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(2), sub2.recv())
        .await
        .expect("该收到 Undo 结果")
        .expect("事件流不该在这里结束");
    assert!(
        matches!(frame.event, SessionEvent::Undo(UndoOutcome::Applied { .. })),
        "重开之后历史该是非空的（否则 undo_turn 只会是 Nothing）：{frame:?}"
    );
}
