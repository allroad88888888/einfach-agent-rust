//! 常驻与独立工具的输入 schema 契约：模型只会看到这里声明的参数形状，所以必须
//! 闭合、扁平、带明确边界，并在重复序列化时保持逐字节稳定。

use agent_core::ToolSpec;
use agent_tools::{builtin_specs, rg_search_spec, search_files_spec, shell_spec};
use serde_json::{Value, json};

fn builtin_schema(name: &str) -> Value {
    builtin_specs()
        .into_iter()
        .find(|spec| spec.name.as_ref() == name)
        .map(|spec| spec.schema.as_ref().clone())
        .unwrap_or_else(|| panic!("缺少内置工具声明：{name}"))
}

fn assert_closed_flat_object(schema: &Value, expected_fields: &[&str]) {
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["additionalProperties"], json!(false));

    let properties = schema["properties"]
        .as_object()
        .expect("schema 必须有 properties 对象");
    assert_eq!(properties.len(), expected_fields.len());
    for field in expected_fields {
        assert!(properties.contains_key(*field), "schema 缺少参数：{field}");
    }

    for combinator in ["allOf", "anyOf", "oneOf", "$ref"] {
        assert!(
            schema.get(combinator).is_none(),
            "常驻工具 schema 必须保持扁平，不能使用 {combinator}"
        );
    }
}

#[test]
fn every_static_tool_schema_serializes_byte_identically() {
    let mut specs: Vec<ToolSpec> = builtin_specs();
    specs.push(shell_spec());

    for spec in specs {
        let first = serde_json::to_vec(&spec).unwrap();
        let second = serde_json::to_vec(&spec).unwrap();
        assert_eq!(
            first, second,
            "{} 的 schema 进入 prompt，序列化不能漂移",
            spec.name
        );
    }
}

#[test]
fn fs_read_schema_closes_extra_fields_and_bounds_pagination() {
    let schema = builtin_schema("srv:fs/read");
    assert_closed_flat_object(&schema, &["path", "offset", "limit"]);
    assert_eq!(schema["required"], json!(["path"]));
    assert_eq!(schema["properties"]["path"]["type"], json!("string"));
    assert_eq!(
        schema["properties"]["offset"],
        json!({
            "type": "integer",
            "minimum": 1,
            "description": "起始行号（1-based），可选，缺省为 1。"
        })
    );
    assert_eq!(
        schema["properties"]["limit"],
        json!({
            "type": "integer",
            "minimum": 1,
            "description": "最多返回的行数，可选，缺省读到文件末尾。"
        })
    );
}

#[test]
fn fs_list_schema_closes_extra_fields_with_optional_path() {
    let schema = builtin_schema("srv:fs/list");
    assert_closed_flat_object(&schema, &["path"]);
    assert!(schema.get("required").is_none(), "path 必须保持可选");
    assert_eq!(schema["properties"]["path"]["type"], json!("string"));
}

#[test]
fn shell_schema_closes_extra_fields_and_bounds_timeout() {
    let schema = shell_spec().schema.as_ref().clone();
    assert_closed_flat_object(&schema, &["cmd", "timeout_secs"]);
    assert_eq!(schema["required"], json!(["cmd"]));
    assert_eq!(schema["properties"]["cmd"]["type"], json!("string"));
    assert_eq!(
        schema["properties"]["timeout_secs"],
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": 300,
            "description": "超时秒数，可选，缺省 30，范围 1~300。"
        })
    );
}

#[test]
fn search_files_schema_is_closed_bounded_and_serialization_stable() {
    let spec = search_files_spec();
    let schema = spec.schema.as_ref();
    assert_closed_flat_object(schema, &["query", "path", "max_results"]);
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(
        schema["properties"]["query"],
        json!({ "type": "string", "minLength": 1, "maxLength": 512,
            "description": "必填：文件名子串；含 * 或 ? 时按完整文件名 glob 匹配。" })
    );
    assert_eq!(
        schema["properties"]["max_results"],
        json!({ "type": "integer", "minimum": 1, "maximum": 1000, "default": 100,
            "description": "可选：最多返回的路径数。" })
    );

    assert_eq!(
        serde_json::to_vec(&spec).unwrap(),
        serde_json::to_vec(&spec).unwrap()
    );
}

#[test]
fn rg_search_schema_is_closed_bounded_and_serialization_stable() {
    let spec = rg_search_spec();
    let schema = spec.schema.as_ref();
    assert_closed_flat_object(schema, &["query", "path", "max_results", "max_line_chars"]);
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(
        schema["properties"]["query"],
        json!({ "type": "string", "minLength": 1, "maxLength": 4096,
            "description": "必填：按字面匹配的文本 needle，不支持正则。" })
    );
    assert_eq!(
        schema["properties"]["max_results"],
        json!({ "type": "integer", "minimum": 1, "maximum": 1000, "default": 200,
            "description": "可选：最多返回的匹配行数。" })
    );
    assert_eq!(
        schema["properties"]["max_line_chars"],
        json!({ "type": "integer", "minimum": 1, "maximum": 4096, "default": 400,
            "description": "可选：每条结果 text 的最大字符数，超出时带 …。" })
    );

    assert_eq!(
        serde_json::to_vec(&spec).unwrap(),
        serde_json::to_vec(&spec).unwrap()
    );
}
