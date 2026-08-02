//! Kimi 的非流式响应体 → 中立的 [`Decoded`]。解析机制在 `crate::wire::decode`
//! （三家共用），这里只传这家的 cached 路径——未命中时这个路径整个缺失。

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

    /// Kimi 特有回归：usage 里 cached 路径整个缺失 → `None`，不是 `Some(0)`
    /// （PROVIDERS.md §一：DeepSeek/GLM 未命中给显式 0，Kimi 干脆不带这个字段）。
    #[test]
    fn missing_cached_path_is_none_not_zero() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "好"}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 5}
        });
        let d = decode(&body);
        assert_eq!(d.usage, TokenUsage { prompt: 100, completion: 5, cached: None });
    }

    /// 命中时正常解析出来。
    #[test]
    fn cached_hit_resolves_through_nested_path() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "好"}}],
            "usage": {"prompt_tokens": 5120, "completion_tokens": 20,
                      "prompt_tokens_details": {"cached_tokens": 5120}}
        });
        assert_eq!(decode(&body).usage.cached, Some(5120));
    }
}
