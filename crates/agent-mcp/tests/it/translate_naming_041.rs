//! 041 验收：翻译后 `ToolSpec.name` 必须是 `mcp:<server>/<tool>` 形状。
//!
//! 规格来源：`crates/agent-mcp/src/translate.rs` 模块文档「命名（红线 11）」、
//! `docs/issues/041-mcp-protocol.md` §验收「name 全是 mcp:everything/<t> 形状」
//! 一条。只测规格，不看实现体。

use agent_mcp::{parse_tools_list, translate};
use crate::common::{everything_tools_list_frame, recorded_result};

#[test]
fn all_translated_names_have_mcp_server_tool_shape() {
    let result = recorded_result(everything_tools_list_frame());
    let tools = parse_tools_list(&result).unwrap();

    for tool in &tools {
        let (spec, _rev) = translate(tool, "everything");
        let expected = format!("mcp:everything/{}", tool.name);
        assert_eq!(
            spec.name.as_ref(),
            expected.as_str(),
            "ToolSpec.name 必须是 mcp:<server>/<tool> 形状"
        );
    }
}
