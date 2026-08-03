use super::*;

#[test]
fn standard_readonly_names_and_order_are_stable() {
    let specs = standard_readonly_file_specs();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_ref()).collect();
    assert_eq!(
        names,
        ["read_file", "list_files", "search_files", "rg_search"]
    );
}

#[test]
fn every_standard_readonly_schema_is_a_closed_flat_object() {
    for spec in standard_readonly_file_specs() {
        let schema = &spec.schema;
        assert_eq!(schema["type"], json!("object"), "{}", spec.name);
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{}",
            spec.name
        );
        assert!(schema["properties"].is_object(), "{}", spec.name);
        for unsupported in ["allOf", "anyOf", "oneOf", "$ref"] {
            assert!(
                schema.get(unsupported).is_none(),
                "{} 不可使用 {unsupported}",
                spec.name
            );
        }
    }
}

#[test]
fn read_and_list_only_offer_controls_supported_by_their_backends() {
    let read = read_file_spec();
    assert_eq!(read.schema["required"], json!(["path"]));
    assert_eq!(read.schema["properties"]["offset"]["minimum"], json!(1));
    assert_eq!(read.schema["properties"]["limit"]["minimum"], json!(1));
    assert!(read.description.contains("revision"));
    assert!(read.description.contains("absent:v1"));
    assert!(read.description.contains("1 MiB"));

    let list = list_files_spec();
    assert!(list.schema.get("required").is_none());
    assert_eq!(list.schema["properties"].as_object().unwrap().len(), 1);
    assert!(list.description.contains("srv:fs/list"));
}

#[test]
fn search_schemas_keep_the_existing_hard_result_limits() {
    let file_name_search = search_files_spec();
    assert_eq!(file_name_search.schema["required"], json!(["query"]));
    assert_eq!(
        file_name_search.schema["properties"]["max_results"]["maximum"],
        json!(1_000)
    );
    assert!(file_name_search.description.contains("truncated=true"));

    let text_search = rg_search_spec();
    assert_eq!(text_search.schema["required"], json!(["query"]));
    assert_eq!(
        text_search.schema["properties"]["max_results"]["maximum"],
        json!(1_000)
    );
    assert_eq!(
        text_search.schema["properties"]["max_line_chars"]["maximum"],
        json!(4_096)
    );
    assert!(text_search.description.contains("srv:fs/rg_search"));
}

#[test]
fn declarations_serialize_byte_identically_across_calls() {
    assert_eq!(
        serde_json::to_vec(&standard_readonly_file_specs()).unwrap(),
        serde_json::to_vec(&standard_readonly_file_specs()).unwrap()
    );
}
