//! 回归测试：`SessionRegistry::open` 曾经有一个 TOCTOU——「检查这个 id 是否
//! 已被占用」和「把新 entry 记进表」是两次分开的持锁操作，中间隔着不快的
//! `actor::spawn`（起线程、等握手）。多个线程并发 `open` 同一个 id 时，都能
//! 在检查那一步看到「表里没有」，都真的起了一个 actor 线程，最后一次
//! `insert` 会悄悄覆盖前一个——前面那些线程从此没人 `join`、没人 `close`，
//! 如果落盘路径相同还会有多个线程写同一个文件。
//!
//! 修复是给表加一个 `Slot::Opening` 中间态（`crate::registry` 模块文档），
//! 这里用真实并发（多个 `std::thread`，不是顺序调用）证明：同一个 id 背靠背
//! 发起 N 次 `open`，有且只有一次成功，其余全部拿到明确的「已经在开/开着」
//! 错误，而不是静默覆盖导致的线程泄漏。

mod support;

use std::sync::{Arc, Barrier};
use std::thread;

const CONCURRENT_OPENS: usize = 8;

#[test]
fn only_one_concurrent_open_of_the_same_id_succeeds() {
    let registry = Arc::new(agent_server::SessionRegistry::new());
    // `Barrier` 逼真正的同时起跑——不是「差不多同时」，是全部线程都排队等到
    // 最后一个准备好了才一起放行，最大化真的撞上那个 TOCTOU 窗口的概率。
    let barrier = Arc::new(Barrier::new(CONCURRENT_OPENS));

    let handles: Vec<_> = (0..CONCURRENT_OPENS)
        .map(|i| {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            // 各起各的假 endpoint/tools_root——这条测试不发任何 `Input`，
            // `open` 本身不碰网络，`endpoint` 只是个占位字符串。
            let spec =
                support::open_spec("same-id", format!("http://127.0.0.1:1/unused-{i}"), None);
            thread::spawn(move || {
                barrier.wait();
                registry.open(spec)
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let ok_count = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        ok_count, 1,
        "并发 open 同一个 id，该有且只有一次成功，实际：{}",
        ok_count
    );

    // 表里现在正好是那一个赢家——`get` 拿到 `Alive`，`close` 只需要一次就能
    // 干净收尾（如果真的泄漏了线程，close 之后 registry 该报的死因/状态不会
    // 是这么干净的 `Ok`）。
    let id = agent_server::SessionId::from("same-id");
    assert!(matches!(
        registry.get(&id),
        Some(agent_server::SessionQuery::Alive(_))
    ));
    assert_eq!(registry.close(&id), Ok(()));
}
