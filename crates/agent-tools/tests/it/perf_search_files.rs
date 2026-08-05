//! `srv:fs/search_files` 的固定规模资源预算：大量长路径的结果必须在工具侧保留
//! 完整 JSON，并受结果数和响应字节数双重限制；不使用 wall-clock 断言。

mod support;

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use support::TestRoot;

const FILE_COUNT: usize = 4_096;
const MAX_RESPONSE_BYTES: usize = 24 * 1024;

#[test]
fn large_file_set_has_deterministic_result_and_response_budgets() {
    let root = TestRoot::new("perf-search-files-budget");
    for index in 0..FILE_COUNT {
        root.write(
            &format!("generated/match-{index:04}-{}.rs", "x".repeat(200)),
            "",
        );
    }

    let executor = ToolExecutor::new(root.path()).unwrap();
    let input = json!({ "query": "*.rs", "max_results": 1000 });
    let first = executor.execute("srv:fs/search_files", &input).unwrap();
    let second = executor.execute("srv:fs/search_files", &input).unwrap();

    assert_eq!(first, second, "固定文件集的枚举结果必须逐字节稳定");
    assert!(first.len() <= MAX_RESPONSE_BYTES);

    let result: Value = serde_json::from_str(&first).unwrap();
    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    assert!(matches.len() <= 1_000, "max_results 必须是硬上限");
    assert!(
        matches.len() < FILE_COUNT,
        "响应字节预算必须在固定大输入下生效"
    );
    assert!(
        matches[0]
            .as_str()
            .unwrap()
            .starts_with("generated/match-0000-")
    );
    assert_eq!(result["truncated"], json!(true));
}
