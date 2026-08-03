//! 有界搜索响应的 JSON 编码。
//!
//! 搜索工具不能依赖 core 的通用截断：后者会把已经生成的 JSON 截为无效文本。
//! 本模块在追加每项之前计算精确的编码字节数，只生成完整、可解析的响应对象。

/// 留给工具结果的最大 UTF-8 字节数；小于 core 的通用输出截断阈值。
pub(crate) const MAX_RESPONSE_BYTES: usize = 24 * 1024;

const PREFIX: &str = r#"{"matches":["#;
const TRUNCATED_SUFFIX: &str = r#"],"truncated":true}"#;
const COMPLETE_SUFFIX: &str = r#"],"truncated":false}"#;

/// 只承载 `matches` 与 `truncated` 的紧凑响应构造器。
pub(crate) struct ResponseBudget {
    entries: Vec<String>,
    entries_bytes: usize,
}

impl ResponseBudget {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            entries_bytes: 0,
        }
    }

    /// 若追加后仍能以 `truncated=true` 形式完整编码，则写入已 JSON 编码的值。
    pub(crate) fn push_encoded(&mut self, encoded: String) -> bool {
        let separator = usize::from(!self.entries.is_empty());
        let total =
            PREFIX.len() + self.entries_bytes + separator + encoded.len() + COMPLETE_SUFFIX.len();
        if total > MAX_RESPONSE_BYTES {
            return false;
        }

        self.entries_bytes += separator + encoded.len();
        self.entries.push(encoded);
        true
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn finish(self, truncated: bool) -> String {
        let suffix = if truncated {
            TRUNCATED_SUFFIX
        } else {
            COMPLETE_SUFFIX
        };
        let result = format!("{PREFIX}{}{suffix}", self.entries.join(","));
        debug_assert!(result.len() <= MAX_RESPONSE_BYTES);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refuses_an_entry_that_would_make_the_json_invalid_after_truncation() {
        let mut response = ResponseBudget::new();
        let encoded = serde_json::to_string(&json!("\u{0001}".repeat(MAX_RESPONSE_BYTES))).unwrap();
        assert!(!response.push_encoded(encoded));
        let result = response.finish(true);
        assert!(result.len() <= MAX_RESPONSE_BYTES);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn reserves_the_larger_complete_suffix_at_the_exact_byte_boundary() {
        let mut response = ResponseBudget::new();
        let entry_len = MAX_RESPONSE_BYTES - PREFIX.len() - COMPLETE_SUFFIX.len();
        let encoded = format!("\"{}\"", "x".repeat(entry_len - 2));
        assert!(response.push_encoded(encoded));

        let result = response.finish(false);
        assert_eq!(result.len(), MAX_RESPONSE_BYTES);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }
}
