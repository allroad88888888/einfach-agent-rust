//! 041 验收：`tools/list` 方法 result 的解析——`parse_tools_list`。工具顺序
//! 必须原样保留（红线 11 的前置条件：翻译层靠这个保序才能保证输出确定）。
//!
//! 规格来源：`crates/agent-mcp/src/protocol.rs`（`McpTool` 字段 +
//! `parse_tools_list` 文档注释）、`docs/issues/041-mcp-protocol.md` §验收
//! 「录制的 tools/list 响应」一条。只测规格，不看实现体。

mod common;

use agent_mcp::{ProtocolError, parse_tools_list};
use common::{everything_tools_list_frame, recorded_result};

#[test]
fn returns_all_tools_in_original_order() {
    let result = recorded_result(everything_tools_list_frame());
    let tools = parse_tools_list(&result).expect("含 4 个工具的 result 应当解析成功");

    assert_eq!(tools.len(), 4);
    // 顺序原样保留（红线 11）：帧里的顺序是 echo, add, printEnv, sendEmail。
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[1].name, "add");
    assert_eq!(tools[2].name, "printEnv");
    assert_eq!(tools[3].name, "sendEmail");

    assert_eq!(tools[0].description.as_deref(), Some("Echoes back the input"));
    assert_eq!(
        tools[0].annotations.as_ref().and_then(|a| a.read_only_hint),
        Some(true)
    );
    // printEnv 没有 annotations 字段。
    assert!(tools[2].annotations.is_none());
}

#[test]
fn missing_tools_array_is_unexpected_shape() {
    let frame = br#"{"jsonrpc":"2.0","id":2,"result":{}}"#;
    let result = recorded_result(frame);
    let err = parse_tools_list(&result).expect_err("result 没有 tools 数组必须报错");
    assert!(
        matches!(err, ProtocolError::UnexpectedShape(_)),
        "应为 UnexpectedShape，实际是 {err:?}"
    );
}
