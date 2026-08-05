//! GLM 的非流式响应体 → 中立的 [`Decoded`]。解析机制在 `crate::wire::decode`
//! （三家共用），这里只传这家的 cached 路径——未命中时这家显式给 0。

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

    /// GLM 特有回归：未命中时 cached 路径**显式给 0** → `Some(0)`，不是 `None`
    /// （PROVIDERS.md §一：Kimi 未命中整个字段缺失，GLM 跟 DeepSeek 一样给 0）。
    #[test]
    fn miss_is_explicit_zero_not_missing() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "好"}}],
            "usage": {"prompt_tokens": 460, "completion_tokens": 3,
                      "prompt_tokens_details": {"cached_tokens": 0}}
        });
        assert_eq!(
            decode(&body).usage,
            TokenUsage {
                prompt: 460,
                completion: 3,
                cached: Some(0)
            }
        );
    }

    #[test]
    fn cached_hit_resolves_through_nested_path() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "好"}}],
            "usage": {"prompt_tokens": 3100, "completion_tokens": 20,
                      "prompt_tokens_details": {"cached_tokens": 3072}}
        });
        assert_eq!(decode(&body).usage.cached, Some(3072));
    }
}
