//! 翻译：一个 [`McpTool`] + server id → 本仓的 `(ToolSpec, Reversibility)`。
//! 这是接缝上「过接缝进本仓」的两样东西里的头两样（见 docs/MCP.md）。
//!
//! # 可逆性的翻译规则（TOOLS.md 钉死，不猜）
//!
//! - `annotations.readOnlyHint == Some(true)` → [`Reversibility::Pure`]
//! - 其余**一律** [`Reversibility::Irreversible`]（`readOnlyHint` 为 `Some(false)`、
//!   `annotations` 缺失、`annotations` 在但无 `readOnlyHint`）
//!
//! 代价不对称：判错成 `Pure` 的代价是重放副作用（重复发邮件/扣款），判错成
//! `Irreversible` 只是多问用户一次。一个未知来源的 MCP 工具默认可重放 = 把数据事故
//! 的开关交给第三方。默认必须落保守边。
//!
//! # 命名（红线 11）
//!
//! `ToolSpec.name` = `mcp:<server_id>/<tool>`——server id 消歧两个 server 的同名工具。
//! `schema` 从 `input_schema` 原样搬（`serde_json::Value` 的 `Map` 是 `BTreeMap`，key 按
//! 字典序排——顶层不开 `preserve_order`，翻译两次逐字节相同，红线 11 靠这个）。

use std::sync::Arc;

use agent_core::{Reversibility, ToolSpec};

use crate::protocol::McpTool;

/// 把一个 MCP 工具翻译成喂模型的 [`ToolSpec`] + 它的 [`Reversibility`]。
/// 规则见模块文档。
pub fn translate(tool: &McpTool, server_id: &str) -> (ToolSpec, Reversibility) {
    let spec = ToolSpec {
        name: Arc::from(format!("mcp:{server_id}/{}", tool.name)),
        // McpTool.description 是 Option；MCP 允许工具不带描述，ToolSpec.description
        // 不是 Option，缺省落空字符串——不用工具名顶替，不能替 server 编话。
        description: Arc::from(tool.description.clone().unwrap_or_default()),
        schema: Arc::new(tool.input_schema.clone()),
    };

    let reversibility = match &tool.annotations {
        Some(annotations) if annotations.read_only_hint == Some(true) => Reversibility::Pure,
        _ => Reversibility::Irreversible,
    };

    (spec, reversibility)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::protocol::Annotations;

    fn tool(name: &str, annotations: Option<Annotations>) -> McpTool {
        McpTool {
            name: name.to_string(),
            description: Some(format!("{name} description")),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            annotations,
        }
    }

    #[test]
    fn name_is_mcp_prefixed_with_server_id() {
        let (spec, _) = translate(&tool("echo", None), "everything");
        assert_eq!(&*spec.name, "mcp:everything/echo");
    }

    #[test]
    fn description_and_schema_carried_over_verbatim() {
        let (spec, _) = translate(&tool("echo", None), "everything");
        assert_eq!(&*spec.description, "echo description");
        assert_eq!(
            *spec.schema,
            json!({"type": "object", "properties": {"path": {"type": "string"}}})
        );
    }

    #[test]
    fn missing_description_becomes_empty_string_not_a_guess() {
        let mut t = tool("echo", None);
        t.description = None;
        let (spec, _) = translate(&t, "everything");
        assert_eq!(&*spec.description, "");
    }

    /// 可逆性翻译穷举——四种 `readOnlyHint` 取值，判错是数据事故（模块文档）。
    #[test]
    fn reversibility_exhaustive_over_read_only_hint() {
        let cases: [(Option<Annotations>, Reversibility); 4] = [
            (
                Some(Annotations {
                    read_only_hint: Some(true),
                }),
                Reversibility::Pure,
            ),
            (
                Some(Annotations {
                    read_only_hint: Some(false),
                }),
                Reversibility::Irreversible,
            ),
            (None, Reversibility::Irreversible),
            (
                Some(Annotations {
                    read_only_hint: None,
                }),
                Reversibility::Irreversible,
            ),
        ];
        for (annotations, expected) in cases {
            let (_, reversibility) = translate(&tool("t", annotations.clone()), "srv");
            assert_eq!(reversibility, expected, "annotations={annotations:?}");
        }
    }

    /// 红线 11：同一个工具翻译两次，`ToolSpec` 序列化逐字节相同。
    #[test]
    fn translate_twice_serializes_byte_identical() {
        let t = tool(
            "echo",
            Some(Annotations {
                read_only_hint: Some(true),
            }),
        );
        let (spec_a, _) = translate(&t, "everything");
        let (spec_b, _) = translate(&t, "everything");
        assert_eq!(
            serde_json::to_vec(&spec_a).unwrap(),
            serde_json::to_vec(&spec_b).unwrap()
        );
    }

    /// 红线 11：`inputSchema` key 集合相同、插入顺序不同的两份，翻译出的 `schema`
    /// 字节相同——证明 `Map` 是 `BTreeMap`，不是随插入顺序走的 `HashMap`/`IndexMap`。
    #[test]
    fn schema_bytes_independent_of_input_schema_insertion_order() {
        let mut map_a = serde_json::Map::new();
        map_a.insert("path".to_string(), json!({"type": "string"}));
        map_a.insert("recursive".to_string(), json!({"type": "boolean"}));

        let mut map_b = serde_json::Map::new();
        map_b.insert("recursive".to_string(), json!({"type": "boolean"}));
        map_b.insert("path".to_string(), json!({"type": "string"}));

        let mut t_a = tool("fs_read", None);
        t_a.input_schema = serde_json::Value::Object(map_a);
        let mut t_b = tool("fs_read", None);
        t_b.input_schema = serde_json::Value::Object(map_b);

        let (spec_a, _) = translate(&t_a, "everything");
        let (spec_b, _) = translate(&t_b, "everything");
        assert_eq!(
            serde_json::to_vec(&spec_a).unwrap(),
            serde_json::to_vec(&spec_b).unwrap()
        );
    }

    /// `tools/list` 顺序原样保留：翻译一个 `Vec<McpTool>` 得到的 `Vec<ToolSpec>` 顺序不变。
    #[test]
    fn translating_a_list_preserves_order() {
        let tools = [tool("b_tool", None), tool("a_tool", None)];
        let names: Vec<String> = tools
            .iter()
            .map(|t| translate(t, "srv").0.name.to_string())
            .collect();
        assert_eq!(
            names,
            vec!["mcp:srv/b_tool".to_string(), "mcp:srv/a_tool".to_string()]
        );
    }
}
