//! `read_loop.rs` 的 wasm 对应物,但**读线程 + `mpsc::sync_channel` 整个不需要
//! 存在**——见 `lib.rs`/issue 113。这里没有阻塞 `read`,所以也没有「给阻塞
//! read 一个外部中断句柄」这个问题要解;`drive_stream` 直接是一个 async 状态
//! 机,取消就是在每次要处理下一行之前看一眼 `cancel`。
//!
//! [`ChunkSource`] 是这份状态机与「字节到底怎么来」之间的接缝:wasm 生产代码
//! 实现一份包住 `web_sys::ReadableStreamDefaultReader` 的版本(见
//! `fetch_client.rs` 的 `WebStreamSource`,那份需要真浏览器/Node 才能跑,本 crate
//! 的测试覆盖不到);本 crate 自己的测试实现一份不碰任何 JS 绑定的
//! `MockChunkSource`,在**任意目标**(包括这次跑 `cargo test -p agent-transport`
//! 的 native)上把完全相同的 `drive_stream` 函数跑一遍,和 native 真正的生产
//! 函数 `read_loop::run` 喂同一份字节比对逐行输出——即“分帧一致性”的证明
//! 不是“我认为两边逻辑等价”,而是同一个 `drive_stream` 函数在两条平台上都会
//! 被调用到,这里验证的是它本身的行为,不是一份平行重新实现。
//!
//! 取消粒度对齐 native:`read_loop::run` 的主循环在每次 `recv_timeout`
//! (即将处理下一个事件)之前查一次 `cancel`;`on_line` 返回 `Break` 立刻停,
//! 不看 cancel。`drive_stream` 在每次要 `await` 下一块字节之前、以及每吐出
//! 一行之前都查 `cancel`,`on_line` 返回 `Break` 同样立刻停——检查点比 native
//! 更密(native 受限于「一次系统调用可能已经吃进多行,只有出 channel 时才能
//! 查」,这里没有这个限制),不会比 native 更迟钝。

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::StreamOutcome;
use crate::line_framer::{FramedLine, LineFramer};

/// wasm 侧「字节从哪儿来」的接缝。生产实现包住一个真实的
/// `ReadableStreamDefaultReader`;测试实现是一个不碰任何 IO/JS 的内存序列。
pub(crate) trait ChunkSource {
    /// 拉取下一块字节。`Ok(None)` 表示流结束——**两种原因不区分**:真正的
    /// EOF(对应 native 的 `LineEvent::Finished`),或者等下一块字节等到一半
    /// 发现 `cancel` 置位、主动放弃了这次等待(生产实现 `WebStreamSource`
    /// 会在返回前先调 `abort()`)。`drive_stream` 收到 `Ok(None)` 后会再看
    /// 一眼 `cancel` 来分辨到底是哪种——这样 `ChunkSource` 不需要为「取消」
    /// 单开一个变体,`MockChunkSource` 之类的简单实现不用关心这一层。
    /// `Err` 表示中途读坏了(对应 native 的 `LineEvent::Broken`)。
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String>;

    /// 通知底层放弃这个连接——对应 native `read_loop::run` 里「取消/Break 时
    /// 立刻返回、不 join 读线程、直接丢弃连接」那句话;wasm 侧这里换成调
    /// `AbortController::abort()`(或者测试里只是记一笔「被喊停了」)。
    fn abort(&mut self);
}

/// 驱动一次流式读:喂 `on_line` 直到流结束、被取消,或中途读坏。跟
/// `read_loop::run` 是同一件事的两种实现,输入输出契约逐条对齐(见模块文档)。
pub(crate) async fn drive_stream<S: ChunkSource>(
    mut source: S,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str) -> ControlFlow<()>,
) -> StreamOutcome {
    let mut framer = LineFramer::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            source.abort();
            return StreamOutcome::Cancelled;
        }
        match source.next_chunk().await {
            Ok(Some(chunk)) => {
                for framed in framer.feed(&chunk) {
                    if cancel.load(Ordering::Relaxed) {
                        source.abort();
                        return StreamOutcome::Cancelled;
                    }
                    match feed_line(framed, &mut on_line) {
                        ControlFlow::Continue(()) => {}
                        ControlFlow::Break(outcome) => {
                            if matches!(outcome, StreamOutcome::Cancelled) {
                                source.abort();
                            }
                            return outcome;
                        }
                    }
                }
            }
            Ok(None) => {
                // 区分「真 EOF」和「等待中发现被取消,提前放弃」——见
                // `ChunkSource::next_chunk` 的文档。`WebStreamSource` 在后一种
                // 情况下已经调过 `abort()` 了,这里不重复调。
                if cancel.load(Ordering::Relaxed) {
                    return StreamOutcome::Cancelled;
                }
                if let Some(framed) = framer.finish()
                    && let ControlFlow::Break(outcome) = feed_line(framed, &mut on_line)
                {
                    // 最后一行触发 Break/非法 UTF-8:流本身已经到 EOF 了,
                    // 没有连接可丢,但结果分类照旧。
                    return outcome;
                }
                return StreamOutcome::Finished;
            }
            Err(message) => return StreamOutcome::Broken(message),
        }
    }
}

/// 把一行 [`FramedLine`] 喂给 `on_line`,翻译成「继续」还是「停在这个
/// outcome」。抽出来只是为了 `drive_stream` 里两个调用点(块内多行、EOF 前
/// 最后一行)不用重复这段翻译逻辑。
fn feed_line(
    framed: FramedLine,
    on_line: &mut impl FnMut(&str) -> ControlFlow<()>,
) -> ControlFlow<StreamOutcome> {
    match framed {
        FramedLine::Line(line) => {
            if on_line(&line).is_break() {
                ControlFlow::Break(StreamOutcome::Cancelled)
            } else {
                ControlFlow::Continue(())
            }
        }
        FramedLine::Invalid(msg) => ControlFlow::Break(StreamOutcome::Broken(msg)),
    }
}
