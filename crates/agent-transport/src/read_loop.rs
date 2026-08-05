//! 流式响应体的按行读循环。**issue 022/023 的硬骨头**：ureq 的阻塞 `read` 没有
//! 外部中断句柄。022 的第一版办法是给 socket 设一个短 read timeout，直接当
//! 「轮询节奏」用；这套在慢首字节的家（Kimi 常见，见 `crate::client` 顶部的
//! 事故记录）上会把「服务端还没吐字节」误判成「连接死了」。
//!
//! 023 把两件事拆开：
//!
//! - socket 的 read timeout 放宽到 `crate::client::DEFAULT_SOCKET_TIMEOUT`
//!   （60s）——只是死流的最终兜底，不再是轮询节奏。
//! - 专门起一个线程做阻塞逐行读，行经 `mpsc::sync_channel` 发给主流程；主流程
//!   `recv_timeout(poll_interval)` 轮询取消标志——**取消的响应速度由这个
//!   间隔决定，跟 socket 的读超时完全解耦**。
//!
//! 取消（标志置位，或 `on_line` 返回 `Break`）时主流程立刻返回，**不 join
//! 读线程**：读线程可能正卡在一次最长 60s 的阻塞 `read_line` 里，join 会把
//! 我们刚解耦掉的问题原样接回来。读线程自己发现 channel 断开（下一次 `send`
//! 失败）就退出、丢弃连接——代价是服务端会继续生成到下一次写失败为止，最多
//! 浪费一条完整响应；这是刻意接受的取舍，M3 的异步取消会把这条尾巴收干净
//! （docs/issues/022-first-provider.md「注意」一节记的原始设计取舍，023 延续
//! 同一个取舍，只是把轮询和死流兜底拆成了两个独立的旋钮）。

use std::io::{BufRead, BufReader, Read};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::StreamOutcome;

/// 读线程往 channel 里发的事件：一行数据，或者流到头的两种终态之一。
enum LineEvent {
    Line(String),
    Finished,
    /// 非超时的 IO 错误，或 60s 死流兜底触发——两者对上层而言是同一件事：
    /// 这个连接不会再有数据了。
    Broken(String),
}

/// 喂 `on_line` 直到流结束、被取消，或连接中途坏掉。`poll_interval` 是主流程
/// 轮询取消标志的节奏（`recv_timeout` 的参数），跟 socket 的读超时无关。
///
/// - 对端正常关闭连接（读到 EOF）→ [`StreamOutcome::Finished`]
/// - 取消标志置位，或 `on_line` 返回 `Break` → [`StreamOutcome::Cancelled`]，
///   立刻返回，不等读线程——已经吐出去的增量收不回来，不重试
/// - 非超时的 IO 错误，或 60s 无新数据的死流兜底 → [`StreamOutcome::Broken`]
pub(crate) fn run(
    reader: Box<dyn Read + Send + Sync>,
    cancel: &AtomicBool,
    poll_interval: Duration,
    mut on_line: impl FnMut(&str) -> ControlFlow<()>,
) -> StreamOutcome {
    // 容量 0（rendezvous）：读线程发一行就等主流程收走，天然背压——读线程不会
    // 在主流程还没处理完上一行时就抢跑攒下一堆行。
    let (tx, rx) = mpsc::sync_channel::<LineEvent>(0);
    thread::spawn(move || read_lines(reader, tx));

    loop {
        if cancel.load(Ordering::Relaxed) {
            return StreamOutcome::Cancelled;
        }
        match rx.recv_timeout(poll_interval) {
            Ok(LineEvent::Line(line)) => {
                let flow = on_line(line.trim_end_matches(['\r', '\n']));
                if flow.is_break() {
                    return StreamOutcome::Cancelled;
                }
            }
            Ok(LineEvent::Finished) => return StreamOutcome::Finished,
            Ok(LineEvent::Broken(msg)) => return StreamOutcome::Broken(msg),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            // 读线程 panic 才会让发送端整体丢弃而不留下终态事件——按连接坏掉
            // 处理，不是取消。
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return StreamOutcome::Broken("读线程异常退出（未留下终态事件）".to_string());
            }
        }
    }
}

/// 读线程本体：阻塞逐行读，每行经 `tx` 发给主流程。`send` 失败说明主流程已经
/// 不要这个连接了（取消/Break 导致接收端被丢弃）——直接退出，不再读下一行。
fn read_lines(reader: Box<dyn Read + Send + Sync>, tx: mpsc::SyncSender<LineEvent>) {
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        match buf_reader.read_line(&mut line) {
            Ok(0) => {
                let _ = tx.send(LineEvent::Finished);
                return;
            }
            Ok(_) => {
                let event = LineEvent::Line(std::mem::take(&mut line));
                if tx.send(event).is_err() {
                    return;
                }
            }
            // 覆盖两种情况：60s 的死流兜底触发（`ErrorKind::TimedOut`），或者
            // 连接中途真的坏了（对端重置、TLS 错误等）。对上层都是 Broken，
            // 不必区分——区分「是不是超时」曾经是轮询节奏的关键信息，现在轮询
            // 节奏已经搬到主流程的 `recv_timeout` 上，这里不再需要 `continue`。
            Err(e) => {
                let _ = tx.send(LineEvent::Broken(e.to_string()));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const FAST_POLL: Duration = Duration::from_millis(20);

    /// 无取消、无超时的直路：逐行喂给回调，EOF 收尾。
    #[test]
    fn feeds_lines_and_reports_finished_on_eof() {
        let data = b"data: a\ndata: b\n".to_vec();
        let reader: Box<dyn Read + Send + Sync> = Box::new(Cursor::new(data));
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let outcome = run(reader, &cancel, FAST_POLL, |line| {
            seen.push(line.to_string());
            ControlFlow::Continue(())
        });
        assert_eq!(seen, vec!["data: a", "data: b"]);
        assert_eq!(outcome, StreamOutcome::Finished);
    }

    /// `on_line` 返回 `Break` 立刻停止，后面的行不再喂。
    #[test]
    fn on_line_break_stops_early() {
        let data = b"data: a\ndata: b\ndata: c\n".to_vec();
        let reader: Box<dyn Read + Send + Sync> = Box::new(Cursor::new(data));
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let outcome = run(reader, &cancel, FAST_POLL, |line| {
            seen.push(line.to_string());
            if line == "data: b" {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(seen, vec!["data: a", "data: b"]);
        assert_eq!(outcome, StreamOutcome::Cancelled);
    }

    /// 取消标志一开始就置位：一行都不喂，立刻返回。
    #[test]
    fn cancel_flag_set_upfront_yields_nothing() {
        let data = b"data: a\n".to_vec();
        let reader: Box<dyn Read + Send + Sync> = Box::new(Cursor::new(data));
        let cancel = AtomicBool::new(true);
        let mut seen = Vec::new();
        let outcome = run(reader, &cancel, FAST_POLL, |line| {
            seen.push(line.to_string());
            ControlFlow::Continue(())
        });
        assert!(seen.is_empty());
        assert_eq!(outcome, StreamOutcome::Cancelled);
    }
}
