//! 041 验收：红线 11——翻译产物（`ToolSpec`）序列化必须逐字节确定，与解析次数
//! 或 wire JSON 里 `inputSchema` 的 key 插入顺序无关。这是工具表进 prompt
//! 最前面、靠前缀缓存省钱的前提；判错了不报错、不 panic，只是每一轮全价
//! （见 docs/INVARIANTS.md 红线 11）。
//!
//! 规格来源：`docs/INVARIANTS.md` 红线 11、
//! `docs/issues/041-mcp-protocol.md` §验收「红线 11」一条、
//! `crates/agent-mcp/src/translate.rs` 模块文档。只测规格，不看实现体。

use agent_mcp::{parse_tools_list, translate};
use crate::common::{everything_tools_list_frame, recorded_result};

#[test]
fn same_tools_list_translated_twice_serializes_byte_identical() {
    let result = recorded_result(everything_tools_list_frame());
    let tools_a = parse_tools_list(&result).unwrap();
    let tools_b = parse_tools_list(&result).unwrap();

    let specs_a: Vec<_> = tools_a
        .iter()
        .map(|t| translate(t, "everything").0)
        .collect();
    let specs_b: Vec<_> = tools_b
        .iter()
        .map(|t| translate(t, "everything").0)
        .collect();

    let bytes_a = serde_json::to_vec(&specs_a).unwrap();
    let bytes_b = serde_json::to_vec(&specs_b).unwrap();
    assert_eq!(
        bytes_a, bytes_b,
        "同一份 tools/list 翻译两次，ToolSpec 序列化必须逐字节相同（红线 11）"
    );
}

#[test]
fn schema_byte_identical_regardless_of_wire_key_insertion_order() {
    // 两份帧：同一个工具，inputSchema 的两个 key（path / recursive）在 wire
    // JSON 里插入顺序相反，其余完全相同。顶层 serde_json 不开
    // preserve_order，Map 是 BTreeMap（key 按字典序排）——翻译出的 schema
    // 必须逐字节相同。
    let frame_order_a = br#"{"jsonrpc":"2.0","id":2,"result":{"tools":[
        {"name":"read","description":"read a file","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"recursive":{"type":"boolean"}}},"annotations":{"readOnlyHint":true}}
    ]}}"#;
    let frame_order_b = br#"{"jsonrpc":"2.0","id":2,"result":{"tools":[
        {"name":"read","description":"read a file","inputSchema":{"type":"object","properties":{"recursive":{"type":"boolean"},"path":{"type":"string"}}},"annotations":{"readOnlyHint":true}}
    ]}}"#;

    let tools_a = parse_tools_list(&recorded_result(frame_order_a)).unwrap();
    let tools_b = parse_tools_list(&recorded_result(frame_order_b)).unwrap();

    let (spec_a, _) = translate(&tools_a[0], "fsserver");
    let (spec_b, _) = translate(&tools_b[0], "fsserver");

    let schema_bytes_a = serde_json::to_vec(&spec_a.schema).unwrap();
    let schema_bytes_b = serde_json::to_vec(&spec_b.schema).unwrap();
    assert_eq!(
        schema_bytes_a, schema_bytes_b,
        "inputSchema key 插入顺序不同，翻译出的 schema 序列化必须逐字节相同（红线 11）"
    );

    // 完整 ToolSpec 序列化（不仅仅是 schema 字段）也必须逐字节相同。
    let full_a = serde_json::to_vec(&spec_a).unwrap();
    let full_b = serde_json::to_vec(&spec_b).unwrap();
    assert_eq!(full_a, full_b);
}
