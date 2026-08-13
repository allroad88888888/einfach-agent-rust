//! 通用 OpenAI 兼容端点的非流式响应体 → 中立的 [`Decoded`]。
//!
//! 解析机制在 `crate::wire::decode`（跟三家共用），这里只传缓存字段路径。
//! **只有一条路径**，不做多路径兜底——理由见 `mod.rs`。

use serde_json::Value;

use super::CACHED_PATHS;
use crate::Decoded;
use crate::wire;

pub(crate) fn decode(body: &Value) -> Decoded {
    wire::decode::decode(body, CACHED_PATHS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::TokenUsage;
    use serde_json::json;

    /// 标准路径命中：174 在 DeepSeek 上实测到的那组数（1280/1301）。
    #[test]
    fn standard_path_hit_resolves() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "ONE"}}],
            "usage": {"prompt_tokens": 1301, "completion_tokens": 26,
                      "prompt_tokens_details": {"cached_tokens": 1280}}
        });
        assert_eq!(
            decode(&body).usage,
            TokenUsage {
                prompt: 1301,
                completion: 26,
                cached: Some(1280)
            }
        );
    }

    /// **字段缺失 ≠ 字段为 0。** 一个什么缓存信息都不给的端点（Ollama 那类本地
    /// 实现大概率如此）必须读成 `None`「不知道」，不能读成 `Some(0)`「确定没命中」
    /// ——读成 0 会让 024 的第 2 层兜底拿到一个**假的对账依据**，
    /// 从此每轮都「预测 0、实际 0、完美吻合」，兜底静默失效。
    #[test]
    fn a_missing_cache_field_is_unknown_not_zero() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "hi"}}],
            "usage": {"prompt_tokens": 42, "completion_tokens": 2}
        });
        assert_eq!(decode(&body).usage.cached, None);
    }

    /// 显式 0 仍然是 `Some(0)`「确定没命中」——跟上面那条是两件事，别合并。
    #[test]
    fn an_explicit_zero_stays_some_zero() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "hi"}}],
            "usage": {"prompt_tokens": 42, "completion_tokens": 2,
                      "prompt_tokens_details": {"cached_tokens": 0}}
        });
        assert_eq!(decode(&body).usage.cached, Some(0));
    }
}
