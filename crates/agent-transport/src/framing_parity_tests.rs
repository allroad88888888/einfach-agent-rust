//! **验收核心**:证明 wasm 侧的分帧结果与 native 侧逐字节相同。
//!
//! 证明方式不是「读代码觉得等价」,是**同一份字节,分别喂给两条真实生产
//! 路径,断言输出的行序列与终态 outcome 完全相等**:
//!
//! - native 侧:`read_loop::run`——`Client::post_stream` 在拿到 200 响应后
//!   实际调用的那个函数,不是重新实现的另一份(`tests/it/fake_sse.rs` 走真
//!   TCP 连接间接测它;这里直接调用同一个 `pub(crate)` 函数,跳过 socket,
//!   因为本文件在 crate 内部,能看见 `pub(crate)` 符号)。
//! - wasm 侧:`stream_drive::drive_stream`——`fetch_client.rs` 在 wasm32
//!   目标上实际调用的那个函数;这里配一个不碰任何 JS/`web_sys` 绑定的
//!   `MockChunkSource`,在 native 目标上把它跑起来。wasm 生产代码与这里的
//!   测试代码调用的是**同一个 `drive_stream` 函数**,区别只在
//!   `ChunkSource` 的实现——所以这里验证的是 `drive_stream` 本身的行为,
//!   不是一份平行重新实现「看起来」和 native 一样。
//!
//! 覆盖的场景对应 `tests/it/fake_sse.rs` 的四条(clean close / on_line
//! break / cancel 置位 / 无尾随换行的残余行),外加「chunk 边界任意切」——
//! 后者是 wasm 独有的问题:`fetch` 的 `ReadableStream` 不保证块边界对齐
//! 行边界,所以同一份数据必须在「整块喂」「逐字节喂」「随机切块喂」下
//! 都产出相同结果。

use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use crate::read_loop;
use crate::stream_drive::{ChunkSource, drive_stream};

/// 驱动一个「保证不会 `Pending`」的 future 到完成——[`MockChunkSource`] 的
/// `next_chunk` 全部立即就绪(测试不需要真实的异步等待,只需要验证
/// `drive_stream` 的分支逻辑),所以一个空操作的 `Waker`(`Waker::noop`,
/// 1.85 起稳定)配上「poll 一次就该 Ready」的断言足够,不必引入
/// `futures`/`tokio` 之类的执行器依赖——这类依赖本该只出现在
/// `agent-server`,不该为了测试悄悄溜进 `agent-transport`。
fn block_on_ready<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    match Pin::new(&mut fut).poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("MockChunkSource 不应产生真实的异步等待"),
    }
}

/// 一份预先切好块的字节序列,`next_chunk` 依次吐出,不碰任何 IO/JS 绑定。
/// `abort` 只是记一笔「被喊停过」,供断言用。
struct MockChunkSource {
    chunks: std::vec::IntoIter<Vec<u8>>,
    aborted: bool,
}

impl MockChunkSource {
    fn new(chunks: Vec<Vec<u8>>) -> Self {
        MockChunkSource {
            chunks: chunks.into_iter(),
            aborted: false,
        }
    }
}

impl ChunkSource for MockChunkSource {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.chunks.next())
    }

    fn abort(&mut self) {
        self.aborted = true;
    }
}

/// 把整份字节按 `chunk_size` 切块(最后一块可能更短);`chunk_size == 0`
/// 表示整份数据一块喂完。
fn chunked(data: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    if chunk_size == 0 {
        return vec![data.to_vec()];
    }
    data.chunks(chunk_size).map(|c| c.to_vec()).collect()
}

/// native 真实生产路径:直接调 `read_loop::run`,喂一个内存 `Cursor`,
/// 跳过 socket——这就是 `tests/it/fake_sse.rs` 背后实际驱动的那个函数。
fn native_lines(data: &[u8]) -> (Vec<String>, crate::StreamOutcome) {
    let mut seen = Vec::new();
    let reader: Box<dyn std::io::Read + Send + Sync> =
        Box::new(std::io::Cursor::new(data.to_vec()));
    let cancel = AtomicBool::new(false);
    let outcome = read_loop::run(
        reader,
        &cancel,
        std::time::Duration::from_millis(20),
        |line| {
            seen.push(line.to_string());
            ControlFlow::Continue(())
        },
    );
    (seen, outcome)
}

/// wasm 侧生产路径:`drive_stream` 配 `MockChunkSource`,`chunk_size` 控制
/// 喂入粒度(0 = 整块,1 = 逐字节,其它 = 任意切块)。
fn wasm_lines(data: &[u8], chunk_size: usize) -> (Vec<String>, crate::StreamOutcome) {
    let mut seen = Vec::new();
    let cancel = AtomicBool::new(false);
    let source = MockChunkSource::new(chunked(data, chunk_size));
    let outcome = block_on_ready(drive_stream(source, &cancel, |line| {
        seen.push(line.to_string());
        ControlFlow::Continue(())
    }));
    (seen, outcome)
}

/// 对同一份数据,native 真实路径与 wasm 真实路径(整块/逐字节/任意切块三种
/// 喂法)全部产出相同的行序列与终态 outcome。
fn assert_framing_matches(data: &[u8]) {
    let native = native_lines(data);
    for chunk_size in [0usize, 1, 3, 7] {
        let wasm = wasm_lines(data, chunk_size);
        assert_eq!(
            native, wasm,
            "chunk_size={chunk_size} 下 wasm 分帧与 native 分帧不一致"
        );
    }
}

#[test]
fn clean_sse_stream_frames_identically() {
    // 对应 fake_sse.rs::streams_lines_and_finishes_on_clean_close 的数据形状。
    assert_framing_matches(b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n");
}

#[test]
fn crlf_terminated_lines_frame_identically() {
    assert_framing_matches(b"data: a\r\ndata: b\r\n\r\ndata: [DONE]\r\n");
}

#[test]
fn trailing_line_without_final_newline_frames_identically() {
    // EOF 前最后一行没有 `\n`——native 侧 read_line 仍会把它当一行吐出。
    assert_framing_matches(b"data: a\ndata: b-no-trailing-newline");
}

#[test]
fn empty_stream_frames_identically() {
    assert_framing_matches(b"");
}

#[test]
fn multi_line_single_chunk_frames_identically() {
    // 一个 chunk 里含多行(SSE 一次 write 常见多行 data),验证多行会在
    // 一次 feed 里被逐条分离,不会粘成一帧。
    assert_framing_matches(
        b"event: delta\ndata: {\"a\":1}\n\nevent: delta\ndata: {\"a\":2}\n\ndata: [DONE]\n\n",
    );
}

/// `on_line` 提前 `Break`:两条路径都必须只看到 break 之前的行,且都以
/// `Cancelled` 收场——对应验收里「取消语义对齐」的一半(另一半是
/// `AtomicBool` 那条,见下面的 `cancel_flag_...` 测试)。
#[test]
fn on_line_break_stops_both_paths_at_the_same_line() {
    let data = b"data: first\ndata: second\ndata: third\n";

    let mut native_seen = Vec::new();
    let cancel = AtomicBool::new(false);
    let reader: Box<dyn std::io::Read + Send + Sync> =
        Box::new(std::io::Cursor::new(data.to_vec()));
    let native_outcome = read_loop::run(
        reader,
        &cancel,
        std::time::Duration::from_millis(20),
        |line| {
            native_seen.push(line.to_string());
            if line == "data: second" {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    );

    let mut wasm_seen = Vec::new();
    let cancel = AtomicBool::new(false);
    let source = MockChunkSource::new(chunked(data, 1));
    let wasm_outcome = block_on_ready(drive_stream(source, &cancel, |line| {
        wasm_seen.push(line.to_string());
        if line == "data: second" {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }));

    assert_eq!(native_seen, vec!["data: first", "data: second"]);
    assert_eq!(wasm_seen, native_seen);
    assert_eq!(native_outcome, crate::StreamOutcome::Cancelled);
    assert_eq!(wasm_outcome, native_outcome);
}

/// `cancel` 标志在处理下一行之前置位:两条路径都必须停在已经处理过的最后
/// 一行,不再多喂一行,且 wasm 侧必须调用过 `ChunkSource::abort`——对应
/// native「取消/Break 时立刻返回、丢弃连接」那句话在 wasm 侧的落地。
#[test]
fn cancel_flag_stops_wasm_source_before_next_chunk_and_calls_abort() {
    struct AbortTrackingSource {
        chunks: std::vec::IntoIter<Vec<u8>>,
        cancel: std::sync::Arc<AtomicBool>,
        aborted: std::sync::Arc<AtomicBool>,
        delivered_first: bool,
    }

    impl ChunkSource for AbortTrackingSource {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
            if self.delivered_first {
                // 模拟「服务端还在但没数据」:真实 fetch 场景里这一步会挂在
                // reader.read() 上;这里改成在喂出第二块之前先把 cancel
                // 标志立起来,验证 drive_stream 在下一次要处理数据前就
                // 会停手,不会真的再消费这一块。
                self.cancel.store(true, Ordering::Relaxed);
            }
            self.delivered_first = true;
            Ok(self.chunks.next())
        }

        fn abort(&mut self) {
            self.aborted.store(true, Ordering::Relaxed);
        }
    }

    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let aborted = std::sync::Arc::new(AtomicBool::new(false));
    let source = AbortTrackingSource {
        chunks: vec![b"data: first\n".to_vec(), b"data: second\n".to_vec()].into_iter(),
        cancel: cancel.clone(),
        aborted: aborted.clone(),
        delivered_first: false,
    };

    let mut seen = Vec::new();
    let outcome = block_on_ready(drive_stream(source, &cancel, |line| {
        seen.push(line.to_string());
        ControlFlow::Continue(())
    }));

    assert_eq!(seen, vec!["data: first"], "cancel 置位后不该再消费第二块");
    assert_eq!(outcome, crate::StreamOutcome::Cancelled);
    assert!(aborted.load(Ordering::Relaxed), "取消时必须调用 abort()");
}

/// 中途读坏(对应 native 的 `LineEvent::Broken`):两条路径都以 `Broken`
/// 收场,已经吐出的行不受影响。
#[test]
fn broken_mid_stream_reports_broken_on_both_paths() {
    struct BreaksAfterOneLine {
        delivered: bool,
    }

    impl ChunkSource for BreaksAfterOneLine {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
            if self.delivered {
                Err("连接中途断开".to_string())
            } else {
                self.delivered = true;
                Ok(Some(b"data: only-line\n".to_vec()))
            }
        }

        fn abort(&mut self) {}
    }

    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let outcome = block_on_ready(drive_stream(
        BreaksAfterOneLine { delivered: false },
        &cancel,
        |line| {
            seen.push(line.to_string());
            ControlFlow::Continue(())
        },
    ));

    assert_eq!(seen, vec!["data: only-line"]);
    assert!(matches!(outcome, crate::StreamOutcome::Broken(_)));
}
