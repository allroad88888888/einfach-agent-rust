//! 按行拆分逻辑，语义逐条对齐 `read_loop.rs` 的真实生产路径
//! （`BufReader::read_line` + `on_line(line.trim_end_matches(['\r','\n']))`）：
//!
//! - 按 `\n` 切分；行内容不含结尾的 `\r`/`\n`。
//! - 流结束（EOF）时若还有尚未见到 `\n` 的残余字节，也当作最后一行吐出——对应
//!   `read_line` 在真正 EOF 之前最后一次返回 `Ok(n>0)` 那一行（native 侧
//!   `read_lines` 循环里 `line` 每次用完就 `mem::take`，所以这一行同样会被
//!   包成 `LineEvent::Line` 送出，不是被吞掉）。
//! - 一整行的字节必须是合法 UTF-8 才能安全 `trim_end_matches`；这点上
//!   `BufReader::read_line` 是**先按 `\n` 收完整行的原始字节，再整体做一次
//!   UTF-8 校验**（不是按每次 `read()` 的字节块边界校验），所以调用方按什么
//!   粒度喂字节进来不影响结果——这正是 wasm 侧要证明的性质：`fetch` 的
//!   `ReadableStream` 按任意字节数切块，谁都不保证块边界和行边界对齐。
//!
//! **native 侧一行没动**：这个模块只服务 wasm 的 fetch 实现，以及下面验证
//! 「wasm 分帧与 native 分帧逐字节相同」的宿主内测试（`framing_parity_tests`）。
//! 后者要把这里的输出和 `read_loop::run`（native 的真实生产函数，不是重新
//! 实现的另一份）做同输入下的逐行比对，所以这个模块只在 wasm32 目标或本 crate
//! 自己的测试里才编译——正常的 native release 构建里它不存在，不会背上
//! 「写了一份没人用的逻辑」的重复实现嫌疑。

/// `LineFramer::feed` / `finish` 吐出的一行。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FramedLine {
    Line(String),
    /// 累积到 `\n` 为止的字节不是合法 UTF-8。对应 `BufReader::read_line`
    /// 校验失败时返回的 `Err`——native 侧 `read_lines` 把它并入
    /// `LineEvent::Broken`，这里同样不单列新的 outcome 种类，交给调用方
    /// （`stream_drive::drive_stream`）翻译成 `StreamOutcome::Broken`。
    Invalid(String),
}

/// 增量喂字节、增量吐行的状态机。字节按任意切法喂进来结果都一样——这是
/// wasm 侧必须具备的性质，因为 `ReadableStream` 的分块边界完全不可控。
#[derive(Default)]
pub(crate) struct LineFramer {
    buf: Vec<u8>,
}

impl LineFramer {
    /// 喂一块新到的字节，吐出这块字节能让缓冲区凑出的所有完整行（可能是
    /// 0 行、1 行，也可能因为这块里含多个 `\n` 而是好几行）。
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<FramedLine> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            out.push(decode_line(&line_bytes));
        }
        out
    }

    /// 流结束（EOF）时调用一次：吐出尚未见到 `\n` 的残余行（如果缓冲区里还有
    /// 字节的话）。对应 native 侧 `read_line` 在真正 EOF 前最后一次返回
    /// `Ok(n>0)` 那一行。
    pub(crate) fn finish(self) -> Option<FramedLine> {
        if self.buf.is_empty() {
            None
        } else {
            Some(decode_line(&self.buf))
        }
    }
}

fn decode_line(bytes: &[u8]) -> FramedLine {
    match std::str::from_utf8(bytes) {
        Ok(s) => FramedLine::Line(s.trim_end_matches(['\r', '\n']).to_string()),
        Err(_) => FramedLine::Invalid("流数据不是合法 UTF-8".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_only(framer_out: Vec<FramedLine>) -> Vec<String> {
        framer_out
            .into_iter()
            .map(|f| match f {
                FramedLine::Line(s) => s,
                FramedLine::Invalid(msg) => panic!("unexpected invalid line: {msg}"),
            })
            .collect()
    }

    #[test]
    fn splits_multiple_lines_in_one_chunk() {
        let mut framer = LineFramer::default();
        let out = framer.feed(b"data: a\ndata: b\n");
        assert_eq!(lines_only(out), vec!["data: a", "data: b"]);
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn byte_by_byte_feed_yields_the_same_lines() {
        let data = b"data: a\ndata: b\n\ndata: [DONE]\n";
        let mut framer = LineFramer::default();
        let mut out = Vec::new();
        for b in data {
            out.extend(framer.feed(std::slice::from_ref(b)));
        }
        assert_eq!(
            lines_only(out),
            vec!["data: a", "data: b", "", "data: [DONE]"]
        );
    }

    #[test]
    fn trailing_partial_line_without_newline_is_flushed_on_finish() {
        let mut framer = LineFramer::default();
        let out = framer.feed(b"data: a\nno newline at eof");
        assert_eq!(lines_only(out), vec!["data: a"]);
        assert_eq!(
            framer.finish(),
            Some(FramedLine::Line("no newline at eof".to_string()))
        );
    }

    #[test]
    fn strips_trailing_cr_from_crlf_lines() {
        let mut framer = LineFramer::default();
        let out = framer.feed(b"data: a\r\ndata: b\r\n");
        assert_eq!(lines_only(out), vec!["data: a", "data: b"]);
    }

    #[test]
    fn empty_input_finishes_with_nothing() {
        let mut framer = LineFramer::default();
        assert!(framer.feed(b"").is_empty());
        assert_eq!(framer.finish(), None);
    }
}
