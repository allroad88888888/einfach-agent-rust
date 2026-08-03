use super::*;

const MAX_DECLARATION_BYTES: usize = 8 * 1024;

#[test]
fn declaration_payload_has_a_fixed_prompt_budget() {
    let first = serde_json::to_vec(&standard_readonly_file_specs()).unwrap();
    let second = serde_json::to_vec(&standard_readonly_file_specs()).unwrap();

    assert_eq!(first, second, "工具声明序列化不能漂移");
    assert!(
        first.len() <= MAX_DECLARATION_BYTES,
        "四个只读工具的 schema+description 不能无界挤占 prompt"
    );
}

#[test]
fn search_results_expose_deterministic_bounds() {
    let read = read_file_spec();
    assert_eq!(read.schema["properties"]["limit"]["minimum"], json!(1));

    let file_name_search = search_files_spec();
    assert_eq!(
        file_name_search.schema["properties"]["max_results"]["default"],
        json!(100)
    );
    assert_eq!(
        file_name_search.schema["properties"]["max_results"]["maximum"],
        json!(1_000)
    );

    let text_search = rg_search_spec();
    assert_eq!(
        text_search.schema["properties"]["max_results"]["default"],
        json!(200)
    );
    assert_eq!(
        text_search.schema["properties"]["max_line_chars"]["default"],
        json!(400)
    );
    assert_eq!(
        text_search.schema["properties"]["max_line_chars"]["maximum"],
        json!(4_096)
    );
}
