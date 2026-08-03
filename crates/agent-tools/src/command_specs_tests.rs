use super::*;

#[test]
fn static_command_names_and_order_are_stable() {
    let specs = command_specs();
    let names: Vec<&str> = specs.iter().map(|spec| &*spec.name).collect();
    assert_eq!(
        names,
        [
            "shell_macos",
            "shell_linux",
            "shell_powershell",
            "run_task",
            "run_verification_command",
            "git_diff_review",
        ]
    );
}

#[test]
fn every_schema_is_a_closed_object() {
    for spec in command_specs() {
        assert_eq!(spec.schema["type"], json!("object"), "{}", spec.name);
        assert_eq!(
            spec.schema["additionalProperties"],
            json!(false),
            "{}",
            spec.name
        );
    }
}

#[test]
fn shell_schemas_share_a_bounded_command_contract() {
    for spec in [
        shell_macos_spec(),
        shell_linux_spec(),
        shell_powershell_spec(),
    ] {
        let schema = &spec.schema;
        assert_eq!(schema["required"], json!(["command"]));
        assert_eq!(schema["properties"]["command"]["minLength"], json!(1));
        assert_eq!(schema["properties"]["command"]["maxLength"], json!(32_768));
        assert_eq!(schema["properties"]["timeout_secs"]["maximum"], json!(120));
        assert_eq!(
            schema["properties"]["max_output_bytes"]["maximum"],
            json!(131_072)
        );
    }
}

#[test]
fn task_schema_only_allows_known_task_kinds() {
    let schema = run_task_spec().schema;
    assert_eq!(schema["required"], json!(["kind"]));
    assert_eq!(
        schema["properties"]["kind"]["enum"],
        json!(["test", "build", "lint", "typecheck", "cargo_check"])
    );
    assert_eq!(schema["properties"]["timeout_secs"]["maximum"], json!(300));
}

#[test]
fn verification_schema_has_no_tunable_execution_knobs() {
    let schema = run_verification_command_spec().schema;
    assert_eq!(schema["required"], json!(["command"]));
    assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
}

#[test]
fn git_diff_schema_exposes_only_read_review_controls() {
    let schema = git_diff_review_spec().schema;
    let fields = schema["properties"].as_object().unwrap();
    assert_eq!(fields.len(), 5);
    assert!(fields.contains_key("paths"));
    assert!(fields.contains_key("staged"));
    assert!(fields.contains_key("base"));
    assert!(fields.contains_key("include_stat"));
    assert!(fields.contains_key("max_output_bytes"));
    assert_eq!(fields["paths"]["maxItems"], json!(100));
}

#[test]
fn declarations_serialize_identically_across_calls() {
    assert_eq!(
        serde_json::to_vec(&command_specs()).unwrap(),
        serde_json::to_vec(&command_specs()).unwrap()
    );
}
