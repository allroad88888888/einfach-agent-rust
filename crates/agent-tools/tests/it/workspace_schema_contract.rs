//! 可撤回工作区工具的 AI 友好输入 schema 契约。

use agent_core::ToolSpec;
use agent_tools::{inspect_spec, revert_change_spec, write_text_spec};
use serde_json::json;

fn assert_closed_flat_object(spec: &ToolSpec, fields: &[&str], required: &[&str]) {
    let schema = spec.schema.as_ref();
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(schema["required"], json!(required));
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(properties.len(), fields.len());
    for field in fields {
        assert!(
            properties.contains_key(*field),
            "{} 缺少 {field}",
            spec.name
        );
    }
    for forbidden in ["allOf", "anyOf", "oneOf", "$ref"] {
        assert!(
            schema.get(forbidden).is_none(),
            "{} 不能使用 {forbidden}",
            spec.name
        );
    }
}

fn assert_stable(spec: ToolSpec) {
    assert_eq!(
        serde_json::to_vec(&spec).unwrap(),
        serde_json::to_vec(&spec).unwrap()
    );
}

#[test]
fn inspect_schema_has_one_explicit_path_input() {
    let spec = inspect_spec();
    assert_closed_flat_object(&spec, &["path"], &["path"]);
    assert_eq!(spec.schema["properties"]["path"]["minLength"], json!(1));
    assert_stable(spec);
}

#[test]
fn write_schema_requires_the_revision_instead_of_allowing_blind_writes() {
    let spec = write_text_spec();
    assert_closed_flat_object(
        &spec,
        &["path", "content", "expected_revision"],
        &["path", "content", "expected_revision"],
    );
    let properties = &spec.schema["properties"];
    assert_eq!(properties["content"]["maxLength"], json!(1_048_576));
    assert_eq!(properties["expected_revision"]["minLength"], json!(9));
    assert_eq!(properties["expected_revision"]["maxLength"], json!(79));
    assert_stable(spec);
}

#[test]
fn revert_schema_accepts_only_a_change_receipt() {
    let spec = revert_change_spec();
    assert_closed_flat_object(&spec, &["change_id"], &["change_id"]);
    assert_eq!(
        spec.schema["properties"]["change_id"]["maxLength"],
        json!(128)
    );
    assert_stable(spec);
}
