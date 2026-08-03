use super::standard_workspace_file_specs;
use serde_json::Value;

#[test]
fn standard_workspace_schemas_are_closed_and_have_a_stable_order() {
    let specs = standard_workspace_file_specs();
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "apply_patch",
            "write_file",
            "delete_path",
            "copy_path",
            "move_path",
            "revert_workspace_change",
        ]
    );
    for spec in specs {
        assert_eq!(
            spec.schema.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "{} must reject invented fields",
            spec.name
        );
    }
}

#[test]
fn path_operations_require_both_observed_revisions() {
    let specs = standard_workspace_file_specs();
    for spec in specs
        .iter()
        .filter(|spec| matches!(spec.name.as_ref(), "copy_path" | "move_path"))
    {
        assert_eq!(
            spec.schema["required"],
            serde_json::json!([
                "source",
                "destination",
                "expected_source_revision",
                "expected_destination_revision"
            ])
        );
    }
}
