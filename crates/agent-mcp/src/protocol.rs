//! MCP 各方法的 params 构造与 result 解析：`initialize`、`tools/list`、`tools/call`。
//! 信封在 `jsonrpc`，翻译成本仓类型在 `translate`。
//!
//! **未知字段一律忽略、不报错**（协议要向前兼容，server 可能带我们还不认识的字段）——
//! 但**该有的字段缺了要报**（`tools/list` 的 result 没有 `tools` 数组是 `UnexpectedShape`）。

use serde_json::{Value, json};

use crate::CLIENT_PROTOCOL_VERSION;
use crate::error::ProtocolError;

/// `initialize` 的 result：协议协商的回值。
#[derive(Debug, Clone, PartialEq)]
pub struct InitializeResult {
    /// server 决定采用的协议版本（可能不同于 client 首选的 [`crate::CLIENT_PROTOCOL_VERSION`]）。
    pub protocol_version: String,
    /// server 声明的能力（`tools`/`resources`/`prompts` 等）。041 原样搬 `Value`，
    /// 具体读哪些位是 042/044 的事。
    pub capabilities: Value,
    /// server 自报的名字（`serverInfo.name`），可能缺。
    pub server_name: Option<String>,
}

/// server 声明的一个工具（`tools/list` result 里的一项）。
///
/// `input_schema` 是 JSON Schema（MCP wire 字段 `inputSchema`），原样搬进 `ToolSpec.schema`。
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub annotations: Option<Annotations>,
}

/// 工具的行为提示（MCP wire 字段 `annotations`）。041 只关心 `readOnlyHint`——它决定
/// 可逆性（见 `translate`）。其余提示位（`destructiveHint`/`idempotentHint` 等）M6 不用，
/// 不解析进来（要用时再加，避免留没人读的字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct Annotations {
    /// `readOnlyHint`：为 `Some(true)` 才翻译成 `Pure`。缺失（`None`）或 `Some(false)`
    /// 都保守落 `Irreversible`——见 `translate` 与 docs/TOOLS.md。
    pub read_only_hint: Option<bool>,
}

/// 构造 `initialize` 的 params（client 首选协议版本 + 能力 + clientInfo）。
pub fn initialize_params(client_name: &str, client_version: &str) -> Value {
    json!({
        "protocolVersion": CLIENT_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": client_name,
            "version": client_version,
        },
    })
}

/// 解析 `initialize` 的 result。未知字段忽略；`protocolVersion` 缺失 → `UnexpectedShape`。
pub fn parse_initialize_result(result: &Value) -> Result<InitializeResult, ProtocolError> {
    let protocol_version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProtocolError::UnexpectedShape("initialize result 缺 protocolVersion".to_string())
        })?
        .to_string();

    // capabilities 缺失时落 `Value::Null`——跟「server 显式声明空能力 `{}`」区分开，
    // 后者是合法值原样搬。
    let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);

    let server_name = result
        .get("serverInfo")
        .and_then(|info| info.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(InitializeResult {
        protocol_version,
        capabilities,
        server_name,
    })
}

/// 解析 `tools/list` 的 result → 工具列表，**顺序原样保留**（红线 11：进 prompt 的东西
/// 逐字节确定，顺序不能靠 HashMap 打乱）。result 里没有 `tools` 数组 → `UnexpectedShape`。
pub fn parse_tools_list(result: &Value) -> Result<Vec<McpTool>, ProtocolError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProtocolError::UnexpectedShape("tools/list result 缺 tools 数组".to_string())
        })?;

    // `Vec` 上的 `.iter()` 按数组原有顺序走——JSON 数组本身就是有序容器，跟顶层
    // `serde_json::Map` 是否 preserve_order 无关，保序不需要额外动作。
    tools
        .iter()
        .enumerate()
        .map(|(index, item)| parse_one_tool(index, item))
        .collect()
}

/// 解析 `tools/list` result 里的一项。`name`/`inputSchema` 缺了报
/// `UnexpectedShape`（下标写进消息方便定位是第几个工具坏了）；`description`/
/// `annotations` 缺失或类型不对一律当成没有，不报错——它们本来就是可选提示位。
fn parse_one_tool(index: usize, item: &Value) -> Result<McpTool, ProtocolError> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError::UnexpectedShape(format!("tools[{index}] 缺 name")))?
        .to_string();

    let description = item
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);

    let input_schema = item
        .get("inputSchema")
        .cloned()
        .ok_or_else(|| ProtocolError::UnexpectedShape(format!("tools[{index}] 缺 inputSchema")))?;

    let annotations = item
        .get("annotations")
        .and_then(Value::as_object)
        .map(|annotations| Annotations {
            read_only_hint: annotations.get("readOnlyHint").and_then(Value::as_bool),
        });

    Ok(McpTool {
        name,
        description,
        input_schema,
        annotations,
    })
}

/// 构造 `tools/call` 的 params（`name` + `arguments`）。`tool_name` 是**裸的 MCP 工具名**
/// （不带 `mcp:<server>/` 前缀——那个前缀是本仓命名，server 不认识；宿主在发之前剥掉）。
pub fn tools_call_params(tool_name: &str, arguments: Value) -> Value {
    json!({
        "name": tool_name,
        "arguments": arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_params_shape() {
        let params = initialize_params("agent-cli", "0.1.0");
        assert_eq!(params["protocolVersion"], json!(CLIENT_PROTOCOL_VERSION));
        assert_eq!(params["capabilities"], json!({}));
        assert_eq!(
            params["clientInfo"],
            json!({"name": "agent-cli", "version": "0.1.0"})
        );
    }

    #[test]
    fn parse_initialize_result_ok() {
        let result = json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "everything", "version": "1.0.0"},
        });
        let parsed = parse_initialize_result(&result).unwrap();
        assert_eq!(parsed.protocol_version, "2025-06-18");
        assert_eq!(parsed.capabilities, json!({"tools": {}}));
        assert_eq!(parsed.server_name, Some("everything".to_string()));
    }

    #[test]
    fn parse_initialize_result_missing_protocol_version_errors() {
        let result = json!({"capabilities": {}});
        assert!(matches!(
            parse_initialize_result(&result).unwrap_err(),
            ProtocolError::UnexpectedShape(_)
        ));
    }

    #[test]
    fn parse_initialize_result_missing_server_name_is_none() {
        let result = json!({"protocolVersion": "2025-06-18", "capabilities": {}});
        let parsed = parse_initialize_result(&result).unwrap();
        assert_eq!(parsed.server_name, None);
    }

    /// 未知字段忽略不报错（协议向前兼容）。
    #[test]
    fn parse_initialize_result_ignores_unknown_fields() {
        let result = json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "instructions": "some text we don't parse in 041",
        });
        assert!(parse_initialize_result(&result).is_ok());
    }

    #[test]
    fn parse_tools_list_ok_preserves_order() {
        let result = json!({
            "tools": [
                {"name": "b_tool", "inputSchema": {"type": "object"}},
                {"name": "a_tool", "inputSchema": {"type": "object"}},
            ]
        });
        let tools = parse_tools_list(&result).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "b_tool");
        assert_eq!(tools[1].name, "a_tool");
    }

    #[test]
    fn parse_tools_list_missing_tools_array_errors() {
        let result = json!({"nope": []});
        assert!(matches!(
            parse_tools_list(&result).unwrap_err(),
            ProtocolError::UnexpectedShape(_)
        ));
    }

    #[test]
    fn parse_tools_list_item_missing_name_errors() {
        let result = json!({"tools": [{"inputSchema": {}}]});
        assert!(matches!(
            parse_tools_list(&result).unwrap_err(),
            ProtocolError::UnexpectedShape(_)
        ));
    }

    #[test]
    fn parse_tools_list_item_missing_input_schema_errors() {
        let result = json!({"tools": [{"name": "echo"}]});
        assert!(matches!(
            parse_tools_list(&result).unwrap_err(),
            ProtocolError::UnexpectedShape(_)
        ));
    }

    #[test]
    fn parse_tools_list_reads_annotations_and_description() {
        let result = json!({
            "tools": [{
                "name": "echo",
                "description": "echoes",
                "inputSchema": {"type": "object"},
                "annotations": {"readOnlyHint": true},
            }]
        });
        let tools = parse_tools_list(&result).unwrap();
        assert_eq!(tools[0].description, Some("echoes".to_string()));
        assert_eq!(
            tools[0].annotations,
            Some(Annotations {
                read_only_hint: Some(true)
            })
        );
    }

    #[test]
    fn parse_tools_list_missing_optional_fields_are_none() {
        let result = json!({"tools": [{"name": "echo", "inputSchema": {}}]});
        let tools = parse_tools_list(&result).unwrap();
        assert_eq!(tools[0].description, None);
        assert_eq!(tools[0].annotations, None);
    }

    #[test]
    fn tools_call_params_shape() {
        let params = tools_call_params("echo", json!({"message": "hi"}));
        assert_eq!(params["name"], json!("echo"));
        assert_eq!(params["arguments"], json!({"message": "hi"}));
    }
}
