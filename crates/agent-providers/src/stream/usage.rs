//! usage 对象 → [`TokenUsage`]，含各家不同的 cached 取值路径。
//!
//! 两处语义在这里焊死，错一处缓存兜底就对不上账（agent-core `TokenUsage::cached`）：
//!
//! - **路径解析不出 = `None`**（这家没报），**解析出 0 = `Some(0)`**（明确没命中）。
//!   实测：Kimi 未命中时整个字段缺失，DeepSeek / GLM 给显式的 0。
//! - usage 帧的位置不固定：可能在 `finish_reason` 同帧，也可能另起一帧且那帧
//!   `choices` 为空（Kimi），还可能挂在 `choices[i].usage` 里（Kimi 的 finish 帧
//!   两处都有）。所以取值**不以 choices 存在为前提**。

use agent_core::TokenUsage;
use serde_json::Value;

/// 从一帧（或一个完整响应体）里找出 usage 对象。先看顶层，再看每个 choice。
/// `"usage": null` 当作没有——DeepSeek 的非尾帧就是这么写的。
pub(crate) fn find(frame: &Value) -> Option<&Value> {
    if let Some(u) = frame.get("usage").filter(|u| u.is_object()) {
        return Some(u);
    }
    frame
        .get("choices")?
        .as_array()?
        .iter()
        .find_map(|c| c.get("usage").filter(|u| u.is_object()))
}

/// usage 对象 → `TokenUsage`。缺失的计数当 0，缺失的 cached 路径当 `None`。
pub(crate) fn parse(usage: &Value, cached_paths: &[&[&str]]) -> TokenUsage {
    TokenUsage {
        prompt: u32_at(usage, "prompt_tokens"),
        completion: u32_at(usage, "completion_tokens"),
        cached: cached_paths.iter().find_map(|path| walk(usage, path)),
    }
}

fn u32_at(usage: &Value, key: &str) -> u32 {
    usage
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// 按路径逐级下钻，任何一级不存在或末级不是整数 → `None`（= 这家没报）。
fn walk(usage: &Value, path: &[&str]) -> Option<u32> {
    let mut cur = usage;
    for key in path {
        cur = cur.get(key)?;
    }
    u32::try_from(cur.as_u64()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DEEPSEEK: &[&[&str]] = &[&["prompt_cache_hit_tokens"]];
    const KIMI: &[&[&str]] = &[&["prompt_tokens_details", "cached_tokens"]];

    /// 录制的 DeepSeek 尾帧：未命中时字段**在**且为 0 → `Some(0)`，不是 `None`。
    #[test]
    fn deepseek_miss_is_some_zero() {
        let u = json!({
            "prompt_tokens": 18, "completion_tokens": 15, "total_tokens": 33,
            "prompt_tokens_details": {"cached_tokens": 0},
            "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 18
        });
        let parsed = parse(&u, DEEPSEEK);
        assert_eq!(parsed.prompt, 18);
        assert_eq!(parsed.completion, 15);
        assert_eq!(parsed.cached, Some(0));
    }

    /// 路径整个不存在 → `None`（这家没报），跟 `Some(0)` 是两码事。
    #[test]
    fn missing_path_is_none() {
        let u = json!({"prompt_tokens": 110, "completion_tokens": 61});
        assert_eq!(parse(&u, DEEPSEEK).cached, None);
        assert_eq!(parse(&u, KIMI).cached, None);
    }

    #[test]
    fn nested_path_resolves() {
        let u = json!({
            "prompt_tokens": 110, "completion_tokens": 61,
            "prompt_tokens_details": {"cached_tokens": 110}
        });
        assert_eq!(parse(&u, KIMI).cached, Some(110));
    }

    /// usage 可能在顶层、可能挂在 choice 上、也可能是 `null`（当没有）。
    #[test]
    fn find_covers_three_positions() {
        assert!(find(&json!({"usage": {"prompt_tokens": 1}})).is_some());
        assert!(find(&json!({"choices": [{"usage": {"prompt_tokens": 1}}]})).is_some());
        assert!(find(&json!({"choices": [{"delta": {}}], "usage": null})).is_none());
        assert!(find(&json!({"choices": []})).is_none());
    }
}
