//! 工具结果的大小上限与截断（决策 19，issue 004）。
//!
//! 一个工具返回 10MB 会顺着 `Messages` 进下一轮 prompt——不做这件事，第一次有人写
//! `grep -r` 工具就炸，而且是「昨天还好好的」那种炸。
//!
//! **截断在 core 边界做**：`ToolResult` 进消息历史之前。executor 不该知道 prompt
//! 预算——它先拿到结果，但预算是 loop 的事。
//!
//! **标记会进 prompt，所以必须逐字节确定**（红线 11）：内容只依赖输入本身
//! （原始大小、保留大小），没有时间戳、没有随机 id。

use std::borrow::Cow;

/// 默认上限：32 KiB ≈ 8k 英文 token / 7–10k 中文 token。
///
/// 一次工具调用最多吃掉 128k 窗口的约 8%；`fs/read` 带行范围参数，超限时模型可以
/// 缩小范围分次拿。可配置，这里只是默认值——取值理由见 ROADMAP 决策 19。
pub const DEFAULT_TOOL_OUTPUT_BYTES: usize = 32 * 1024;

/// 截断到 `limit` 字节（UTF-8 安全），超限时**只保留头部**并追加可见标记。
///
/// 只留头部是拍板过的取舍：文件读取和 grep 的头部信息密度最高，实现可预测；
/// M2 的 `shell/exec`（错误常在输出尾部）到 issue 020 再议要不要改。
///
/// 保留的内容正好 `limit` 字节（向下取到字符边界），标记追加在其后——总长会比
/// `limit` 多出标记那几十字节。这是刻意的：「上限」约束的是失控的工具输出，
/// 不是标记本身；让内容预算恒等于 `limit` 比「内容 + 标记 ≤ limit」好预测。
///
/// 标记必须让模型知道三件事：看到的是残缺的、原来有多大、怎么拿更多——
/// 否则它会基于不完整的数据下结论。
pub fn truncate_tool_output(s: &str, limit: usize) -> Cow<'_, str> {
    if s.len() <= limit {
        return Cow::Borrowed(s);
    }
    let cut = truncated_content_bytes(s, limit);
    let kept = &s[..cut];
    Cow::Owned(format!(
        "{kept}\n\n[输出被截断：原始 {} 字节，仅保留前 {} 字节。需要其余部分请缩小范围重新调用（如 fs/read 的行范围）。]",
        s.len(),
        cut,
    ))
}

/// [`truncate_tool_output`] 实际保留的**纯内容**字节数（不含截断标记）。
///
/// 单独暴露是因为 `Notice::ToolOutputTruncated.kept_bytes` 要的正是这个数，而
/// `truncate_tool_output` 返回的 `Cow::Owned` 已经把标记拼在内容后面（标记文本里
/// 嵌了两个变长数字，没法按固定偏移切回去）。转移表以前各自抄了一份同样的
/// 「向下找字符边界」算法——两份一旦分叉，`kept_bytes` 报的就不是模型真正看到的
/// 字节数，而且不报错。026 收成这一处。
///
/// 未超限时返回 `s.len()`（什么都没截）。
pub fn truncated_content_bytes(s: &str, limit: usize) -> usize {
    if s.len() <= limit {
        return s.len();
    }
    // 向下找 UTF-8 字符边界，多字节字符（中文 3 字节）不能从中间切开。
    let mut cut = limit;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_limit_untouched() {
        let s = "短输出";
        let out = truncate_tool_output(s, 1024);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, s);
    }

    #[test]
    fn exactly_at_limit_untouched() {
        let s = "a".repeat(100);
        assert!(matches!(truncate_tool_output(&s, 100), Cow::Borrowed(_)));
    }

    #[test]
    fn over_limit_keeps_head_and_marks() {
        let s = "x".repeat(200);
        let out = truncate_tool_output(&s, 100);
        assert!(out.starts_with(&"x".repeat(100)));
        assert!(
            !out.contains(&"x".repeat(101)),
            "内容部分必须正好 limit 字节"
        );
        assert!(out.contains("原始 200 字节"));
        assert!(out.contains("保留前 100 字节"));
        assert!(out.contains("缩小范围重新调用"));
    }

    #[test]
    fn cuts_on_char_boundary() {
        // 中文每字 3 字节；limit 落在字符中间时必须往回退到边界。
        let s = "汉".repeat(100); // 300 字节
        for limit in [100, 101, 102] {
            let out = truncate_tool_output(&s, limit);
            // 能构造 &str 本身就证明没切坏 UTF-8；再验内容是整字。
            let head = out.split('\n').next().unwrap();
            assert_eq!(head.len() % 3, 0, "limit={limit} 时切在了字符中间");
            assert!(head.len() <= limit);
        }
    }

    #[test]
    fn deterministic() {
        // 红线 11：标记进 prompt，两次调用必须逐字节相同。
        let s = "y".repeat(50_000);
        let a = truncate_tool_output(&s, DEFAULT_TOOL_OUTPUT_BYTES);
        let b = truncate_tool_output(&s, DEFAULT_TOOL_OUTPUT_BYTES);
        assert_eq!(a, b);
    }

    #[test]
    fn ten_megabytes_bounded() {
        // 004 的验收原文：10MB 的工具结果不会让下一轮 prompt 爆炸。
        let s = "z".repeat(10 * 1024 * 1024);
        let out = truncate_tool_output(&s, DEFAULT_TOOL_OUTPUT_BYTES);
        assert!(out.len() < DEFAULT_TOOL_OUTPUT_BYTES + 200);
    }
}
