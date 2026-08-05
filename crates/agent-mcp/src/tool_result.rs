//! `tools/call` 的 result 拍平成一条工具结果：可见文本 + `isError`。
//!
//! 这是「过接缝进本仓」的第三样在执行期的落点：043 的执行路
//! （`agent-runtime::mcp_call`）拿 [`ToolCallOutput`] 直接组
//! `Event::ToolResult`/`ToolFailed`。**MCP wire 形状（`content` 块数组、
//! `isError`）留在本 crate 里**，runtime 只接一段文本 + 一个布尔，`agent-runtime`
//! grep 不到任何 MCP 结构（docs/MCP.md §接缝：wire 类型不过接缝）。

use serde_json::Value;

/// 一次 `tools/call` 的结果拍平后的样子：喂回模型的文本 + 它是不是错误。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallOutput {
    pub text: String,
    pub is_error: bool,
}

/// 拍平一个 `tools/call` result。`isError` 缺省为 `false`（协议默认成功）；`content`
/// 逐块取 `text` 拼接。**没有 text 块**（image/resource 等 M6 未翻译的块）时不喂空串
/// ——原样搬 `content` 的 JSON，保守不丢信息；`content` 整个缺失（不合规）则搬整个
/// result。这一步不判可逆性、不判成功失败之外的语义，只做「wire → 文本」。
pub fn flatten_tool_result(result: &Value) -> ToolCallOutput {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = match result.get("content").and_then(Value::as_array) {
        Some(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if texts.is_empty() {
                Value::Array(blocks.clone()).to_string()
            } else {
                texts.join("\n")
            }
        }
        None => result.to_string(),
    };
    ToolCallOutput { text, is_error }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn joins_text_blocks_and_reads_is_error() {
        let ok = flatten_tool_result(&json!({
            "content": [{"type": "text", "text": "line 1"}, {"type": "text", "text": "line 2"}],
        }));
        assert_eq!(
            ok,
            ToolCallOutput {
                text: "line 1\nline 2".to_string(),
                is_error: false
            }
        );

        let err = flatten_tool_result(&json!({
            "content": [{"type": "text", "text": "boom"}],
            "isError": true,
        }));
        assert_eq!(
            err,
            ToolCallOutput {
                text: "boom".to_string(),
                is_error: true
            }
        );
    }

    /// `isError` 缺省成功；未知字段忽略。
    #[test]
    fn missing_is_error_defaults_to_success() {
        let out = flatten_tool_result(&json!({
            "content": [{"type": "text", "text": "ok"}],
            "someUnknownField": 1,
        }));
        assert!(!out.is_error);
        assert_eq!(out.text, "ok");
    }

    /// 没有 text 块不喂空串——搬 content 的 JSON，保守不丢信息。
    #[test]
    fn without_text_blocks_falls_back_to_content_json() {
        let out = flatten_tool_result(&json!({"content": [{"type": "image", "data": "..."}]}));
        assert!(!out.is_error);
        assert!(
            out.text.contains("image"),
            "该保留 content 原样：{}",
            out.text
        );
    }
}
