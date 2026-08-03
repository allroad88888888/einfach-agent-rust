//! `srv:fs/search_files` 的功能与确定性输出预算。

mod support;

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use support::TestRoot;

fn run_search(root: &TestRoot, input: Value) -> Value {
    serde_json::from_str(&run_raw(root, input)).unwrap()
}

fn run_raw(root: &TestRoot, input: Value) -> String {
    ToolExecutor::new(root.path())
        .unwrap()
        .execute("srv:fs/search_files", &input)
        .unwrap()
}

#[test]
fn finds_substrings_and_file_name_globs_in_stable_relative_order() {
    let root = TestRoot::new("search-files-happy");
    root.write("src/zeta.rs", "");
    root.write("src/alpha.rs", "");
    root.write("tests/alpha_test.rs", "");
    root.write("README.md", "");

    let glob = run_search(&root, json!({ "query": "*.rs", "path": "src" }));
    assert_eq!(
        glob,
        json!({
            "matches": ["src/alpha.rs", "src/zeta.rs"],
            "truncated": false,
        })
    );

    let substring = run_search(&root, json!({ "query": "alpha" }));
    assert_eq!(
        substring,
        json!({
            "matches": ["src/alpha.rs", "tests/alpha_test.rs"],
            "truncated": false,
        })
    );
}

#[cfg(unix)]
#[test]
fn skips_symlinked_file_entries() {
    let root = TestRoot::new("search-files-symlink");
    root.write("actual.rs", "");
    std::os::unix::fs::symlink(root.path().join("actual.rs"), root.path().join("linked.rs"))
        .unwrap();

    let result = run_search(&root, json!({ "query": "*.rs" }));
    assert_eq!(
        result,
        json!({ "matches": ["actual.rs"], "truncated": false })
    );
}

#[test]
fn max_results_is_a_deterministic_output_budget() {
    let root = TestRoot::new("search-files-budget");
    for index in 0..128 {
        root.write(&format!("generated/match-{index:03}.rs"), "");
    }

    let first = run_search(&root, json!({ "query": "*.rs", "max_results": 16 }));
    let second = run_search(&root, json!({ "query": "*.rs", "max_results": 16 }));
    assert_eq!(first, second, "遍历顺序不能依赖文件系统返回顺序");
    assert_eq!(first["matches"].as_array().unwrap().len(), 16);
    assert_eq!(first["matches"][0], json!("generated/match-000.rs"));
    assert_eq!(first["matches"][15], json!("generated/match-015.rs"));
    assert_eq!(first["truncated"], json!(true));
    assert!(serde_json::to_string(&first).unwrap().len() < 1_000);
}

#[test]
fn exact_result_limit_is_complete_when_no_additional_match_exists() {
    let root = TestRoot::new("search-files-exact-limit");
    for name in ["a.rs", "b.rs", "c.rs"] {
        root.write(name, "");
    }

    let result = run_search(&root, json!({ "query": "*.rs", "max_results": 3 }));
    assert_eq!(result["matches"].as_array().unwrap().len(), 3);
    assert_eq!(result["truncated"], json!(false));
}

#[test]
fn maximum_user_result_count_still_produces_complete_json_within_byte_budget() {
    let root = TestRoot::new("search-files-json-budget");
    for index in 0..200 {
        root.write(&format!("generated/{index:03}-{}.rs", "x".repeat(220)), "");
    }

    let input = json!({ "query": "*.rs", "max_results": 1000 });
    let first = run_raw(&root, input.clone());
    let second = run_raw(&root, input);
    assert_eq!(first, second, "响应预算也必须保持确定性");
    assert!(first.len() <= 24 * 1024);

    let result: Value = serde_json::from_str(&first).unwrap();
    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    assert!(matches.len() < 200);
    assert_eq!(result["truncated"], json!(true));
}

#[test]
fn rejects_unknown_arguments_and_paths_outside_root() {
    let root = TestRoot::new("search-files-invalid");
    let executor = ToolExecutor::new(root.path()).unwrap();

    let unknown = executor
        .execute(
            "srv:fs/search_files",
            &json!({ "query": "a", "unexpected": true }),
        )
        .unwrap_err();
    assert_eq!(&*unknown.code, "bad_input");

    let outside = executor
        .execute(
            "srv:fs/search_files",
            &json!({ "query": "a", "path": "../" }),
        )
        .unwrap_err();
    assert_eq!(&*outside.code, "outside_root");
}
