//! 041 验收：可逆性翻译规则的穷举——`readOnlyHint` 的四种取值组合。判错成
//! `Pure` 是数据事故（重放副作用：重复发邮件/扣款），所以这条测试由不碰实现
//! 的独立测试 agent 写、且必须穷举，不能只测「大概率会被想到」的那两种。
//!
//! 规则（docs/MCP.md §翻译规则，`crates/agent-mcp/src/translate.rs` 模块
//! 文档钉死）：
//! - `annotations.readOnlyHint == Some(true)` → `Reversibility::Pure`
//! - 其余一律 `Reversibility::Irreversible`：`Some(false)` / `annotations`
//!   缺失 / `annotations` 在但无 `readOnlyHint`
//!
//! 规格来源：`docs/issues/041-mcp-protocol.md` §验收「可逆性翻译穷举」一条、
//! `docs/MCP.md` §「可逆性不能再从名字推」。只测规格，不看实现体。

mod common;

use agent_core::Reversibility;
use agent_mcp::{Annotations, parse_tools_list, translate};
use common::{recorded_result, tool_with_annotations};

#[test]
fn read_only_true_is_pure() {
    let tool = tool_with_annotations(
        "t",
        Some(Annotations {
            read_only_hint: Some(true),
        }),
    );
    let (_, rev) = translate(&tool, "s");
    assert_eq!(
        rev,
        Reversibility::Pure,
        "readOnlyHint:true 必须翻译成 Pure"
    );
}

#[test]
fn read_only_false_is_irreversible() {
    let tool = tool_with_annotations(
        "t",
        Some(Annotations {
            read_only_hint: Some(false),
        }),
    );
    let (_, rev) = translate(&tool, "s");
    assert_eq!(
        rev,
        Reversibility::Irreversible,
        "readOnlyHint:false 必须翻译成 Irreversible"
    );
}

#[test]
fn missing_annotations_is_irreversible() {
    let tool = tool_with_annotations("t", None);
    let (_, rev) = translate(&tool, "s");
    assert_eq!(
        rev,
        Reversibility::Irreversible,
        "annotations 缺失必须翻译成 Irreversible（保守默认）"
    );
}

#[test]
fn annotations_present_without_hint_is_irreversible() {
    let tool = tool_with_annotations(
        "t",
        Some(Annotations {
            read_only_hint: None,
        }),
    );
    let (_, rev) = translate(&tool, "s");
    assert_eq!(
        rev,
        Reversibility::Irreversible,
        "annotations 在但无 readOnlyHint 必须翻译成 Irreversible（保守默认）"
    );
}

/// 同一穷举矩阵再走一遍完整录制帧 → parse_tools_list → translate 的路径，
/// 确认 wire 层的 4 种 annotations 形状真的能被 `parse_tools_list` 正确解析
/// 成上面 4 个单测断言的 `McpTool` 形状（不只是 `translate()` 单独测对，
/// parse 层把 annotations 解析错也会在这里暴露）。
#[test]
fn exhaustive_matrix_also_holds_via_recorded_frame() {
    let frame = br#"{"jsonrpc":"2.0","id":2,"result":{"tools":[
        {"name":"readOnlyTrue","description":"d","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},
        {"name":"readOnlyFalse","description":"d","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":false}},
        {"name":"noAnnotations","description":"d","inputSchema":{"type":"object"}},
        {"name":"annotationsNoHint","description":"d","inputSchema":{"type":"object"},"annotations":{"title":"something"}}
    ]}}"#;
    let tools = parse_tools_list(&recorded_result(frame)).unwrap();
    assert_eq!(tools.len(), 4);

    assert_eq!(translate(&tools[0], "everything").1, Reversibility::Pure);
    assert_eq!(
        translate(&tools[1], "everything").1,
        Reversibility::Irreversible
    );
    assert_eq!(
        translate(&tools[2], "everything").1,
        Reversibility::Irreversible
    );
    assert_eq!(
        translate(&tools[3], "everything").1,
        Reversibility::Irreversible
    );
}
