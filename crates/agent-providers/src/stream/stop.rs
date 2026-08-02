//! `finish_reason` 字符串 → [`StopReason`]。
//!
//! 三家的流式骨架都是 OpenAI 兼容（probes/PROVIDERS.md §三），这张词表因此是
//! **数据不是行为**，共享一份；流式收尾和非流式 `decode` 都走这里，两条路径不
//! 允许对同一个字符串给出不同的结论。
//!
//! **认不出的一律 [`StopReason::Other`]，绝不猜成 `EndTurn`**——猜错了 loop 会
//! 以为轮次正常结束，实际上可能是被截断或者后端资源不足
//! （DeepSeek 有 `insufficient_system_resource` 这一类取值）。

use std::sync::Arc;

use agent_core::StopReason;

/// 字段整个缺失时用的取值。流里没等到 `finish_reason` 就断了、或响应体里压根
/// 没这个字段，都落到这里——它和「认不出的取值」一样是 `Other`，因为两者对
/// 上层的意义相同：**这轮不能当作正常结束**。
pub(crate) fn missing() -> StopReason {
    StopReason::Other(Arc::from("missing"))
}

pub(crate) fn from_wire(raw: &str) -> StopReason {
    match raw {
        "stop" => StopReason::EndTurn,
        // `function_call` 是 OpenAI 系的老取值，语义同 `tool_calls`。
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" | "max_tokens" => StopReason::MaxTokens,
        // OpenAI 系把「撞上 stop 序列」也报成 `stop`，三家实测都没有独立取值；
        // 留这一条是因为词表是 core 定的，哪家真报了才不至于掉进 `Other`。
        "stop_sequence" => StopReason::StopSequence,
        other => StopReason::Other(Arc::from(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(from_wire("stop"), StopReason::EndTurn);
        assert_eq!(from_wire("tool_calls"), StopReason::ToolUse);
        assert_eq!(from_wire("length"), StopReason::MaxTokens);
    }

    /// 未知取值原样落进 `Other`，**不许**变成 `EndTurn`。
    #[test]
    fn unknown_never_becomes_end_turn() {
        for raw in ["insufficient_system_resource", "content_filter", ""] {
            assert_eq!(from_wire(raw), StopReason::Other(Arc::from(raw)));
        }
        assert_eq!(missing(), StopReason::Other(Arc::from("missing")));
    }
}
