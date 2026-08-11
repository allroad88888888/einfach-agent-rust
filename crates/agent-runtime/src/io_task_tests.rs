//! 117 的三条硬性质，全部不碰 HTTP：[`run`] 只吃一串 [`StreamItem`]，测试直接
//! 手工喂。
//!
//! 1. **保底槽位**——`DoneDebt` 的整条论证（「`try_send` 只可能因为接收端没了而
//!    失败」）建立在 `futures` 的容量公式 `buffer + sender 数` 上。那是别人家
//!    库的实现细节，不写测试就是把「泵会不会为一个已经没了的调用永远等下去」
//!    押在一段注释上。
//! 2. **欠债—还债**——future 半路被丢掉时泵仍然收得到终态消息（验收第三条）。
//! 3. **会合背压的上界**——115 拍板接受「每个发送端最多缓冲 1 条」，这里把
//!    「1」焊死：多喂几行，泵不收，载体也只会跑在前面一条。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use agent_core::AgentId;
use agent_providers::{Provider, deepseek::DeepSeek};
use agent_transport::StreamOutcome;

use super::*;

/// 一条 DeepSeek wire 上的文本增量帧。
fn text_line(text: &str) -> String {
    format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
    )
}

struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
    fn wake_by_ref(self: &Arc<Self>) {}
}

/// 推一次 future。返回值只用来判断它有没有跑完——测试关心的是它**这一推**把
/// 什么写进了 channel。
fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::new(Noop));
    future.as_mut().poll(&mut Context::from_waker(&waker))
}

fn task_under_test(
    tx: &IoSender,
    attempt: ProviderAttemptId,
    items: mpsc::Receiver<StreamItem>,
) -> Pin<Box<impl Future<Output = ()>>> {
    Box::pin(run(
        tx.clone(),
        tx.clone(),
        AgentId::root(),
        attempt,
        DeepSeek.accumulator(),
        items,
    ))
}

/// `DoneDebt::settle`/`Drop` 用的是 `try_send`（`Drop` 里没有 `.await` 可用）。
/// 它凭什么不会失败：`futures` 的有界 channel 容量是 `buffer + sender 数`，
/// **槽位按 sender 记**。所以一份「一辈子只发一条」的 clone 永远有位置。
#[test]
fn a_fresh_sender_always_has_one_slot_even_when_the_channel_is_full() {
    let (tx, mut rx) = mpsc::channel::<IoMsg>(0);
    let mut deltas = tx.clone();
    let mut debt = tx.clone();
    let agent = AgentId::root();
    let attempt = ProviderAttemptId::allocate();

    assert!(
        deltas
            .try_send(IoMsg::Provider(ProviderMessage::gone(
                agent.clone(),
                attempt
            )))
            .is_ok()
    );
    assert!(
        deltas
            .try_send(IoMsg::Provider(ProviderMessage::gone(
                agent.clone(),
                attempt
            )))
            .is_err(),
        "同一个 sender 的第二条该被挡住——115 说的「每个发送端至少缓冲 1 条」，上界就是 1"
    );
    assert!(
        debt.try_send(IoMsg::Provider(ProviderMessage::gone(agent, attempt)))
            .is_ok(),
        "另一份 clone 有自己的保底槽位：DoneDebt 因此不会因为「channel 满了」而还不上债"
    );

    // 两条都真的在队列里，不是被吞了。
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
}

/// 验收第三条：future 在拿到终态之前被丢掉（超时/取消/整轮收工都是这条路），
/// 泵仍然收得到一条终态消息。没有它，泵会为一个已经不存在的调用一直等下去
/// ——**不报错，表现为对话永久转圈**。
#[test]
fn dropping_the_task_before_its_terminal_item_still_pays_the_debt() {
    let (tx, mut rx) = mpsc::channel::<IoMsg>(0);
    let (mut items_tx, items_rx) = mpsc::channel::<StreamItem>(4);
    let attempt = ProviderAttemptId::allocate();
    let mut task = task_under_test(&tx, attempt, items_rx);

    items_tx
        .try_send(StreamItem::Prepared(Vec::new()))
        .expect("行源该发得出准备完成");
    assert!(
        poll_once(&mut task).is_pending(),
        "还没有终态，该停在等下一行"
    );
    assert!(rx.try_recv().is_err(), "准备完成不是给泵看的消息");

    drop(task);

    let message = rx.try_recv().expect("被丢掉的 future 必须还上一条终态消息");
    let IoMsg::Provider(message) = message else {
        panic!("该是一条 provider 消息");
    };
    assert_eq!(message.kind(), "gone");
    assert_eq!(message.attempt(), attempt, "债要认在自己那次 attempt 上");
    assert_eq!(message.agent(), &AgentId::root());
}

/// 正常路径：终态由 `settle` 还，`Drop` 不再补第二条。
#[test]
fn a_settled_task_pays_exactly_one_terminal_message() {
    let (tx, mut rx) = mpsc::channel::<IoMsg>(0);
    let (mut items_tx, items_rx) = mpsc::channel::<StreamItem>(4);
    let attempt = ProviderAttemptId::allocate();
    let mut task = task_under_test(&tx, attempt, items_rx);

    items_tx.try_send(StreamItem::Prepared(Vec::new())).unwrap();
    let _ = poll_once(&mut task);
    items_tx
        .try_send(StreamItem::Done(Ok(StreamOutcome::Finished)))
        .unwrap();
    assert!(poll_once(&mut task).is_ready(), "拿到终态就该收工");

    let IoMsg::Provider(message) = rx.try_recv().expect("该有一条终态消息") else {
        panic!("该是一条 provider 消息");
    };
    assert_eq!(message.kind(), "done");

    drop(task);
    assert!(
        rx.try_recv().is_err(),
        "已经还过的债不能被 Drop 再还一次——那会让泵拿到两条终态"
    );
}

/// 115 决策 3 接受的那条代价，把「1」焊死：泵不收，载体最多跑在前面一条。
/// 变成无界缓冲的话这条会立刻红（三条增量会一次性全部进 channel）。
#[test]
fn the_task_never_runs_more_than_one_delta_ahead_of_the_pump() {
    let (tx, mut rx) = mpsc::channel::<IoMsg>(0);
    let (mut items_tx, items_rx) = mpsc::channel::<StreamItem>(8);
    let attempt = ProviderAttemptId::allocate();
    let mut task = task_under_test(&tx, attempt, items_rx);

    items_tx.try_send(StreamItem::Prepared(Vec::new())).unwrap();
    for text in ["一", "二", "三"] {
        items_tx
            .try_send(StreamItem::Line(text_line(text)))
            .unwrap();
    }

    assert!(poll_once(&mut task).is_pending());
    assert!(rx.try_recv().is_ok(), "第一条增量该已经在 channel 里");
    assert!(
        rx.try_recv().is_err(),
        "第二条必须等泵取走第一条之后才发得出去——这就是会合背压"
    );

    // 取走一条，载体才能再往前走一条。
    assert!(poll_once(&mut task).is_pending());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}
