//! `srv:fs/rg_search` 的固定规模资源预算：最大用户行/结果参数不能放大为超过
//! 工具响应上限的无效 JSON，输出顺序也不得依赖文件系统返回顺序。

mod support;

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use support::TestRoot;

const FILE_COUNT: usize = 512;
const MAX_LINE_CHARS: usize = 4_096;
const MAX_RESPONSE_BYTES: usize = 24 * 1024;

#[test]
fn maximum_search_parameters_keep_json_deterministic_and_byte_bounded() {
    let root = TestRoot::new("perf-rg-search-budget");
    let content = format!("needle{}\n", "x".repeat(MAX_LINE_CHARS - "needle".len()));
    for index in 0..FILE_COUNT {
        root.write(&format!("src/file-{index:04}.txt"), &content);
    }

    let executor = ToolExecutor::new(root.path()).unwrap();
    let input = json!({
        "query": "needle",
        "max_results": 1000,
        "max_line_chars": MAX_LINE_CHARS,
    });
    let first = executor.execute("srv:fs/rg_search", &input).unwrap();
    let second = executor.execute("srv:fs/rg_search", &input).unwrap();

    assert_eq!(first, second, "固定文件集的搜索结果必须逐字节稳定");
    assert!(first.len() <= MAX_RESPONSE_BYTES);

    let result: Value = serde_json::from_str(&first).unwrap();
    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    assert!(matches.len() <= 1_000, "max_results 必须是硬上限");
    assert!(
        matches.len() < FILE_COUNT,
        "响应字节预算必须在固定大输入下生效"
    );
    assert!(matches.iter().all(|hit| {
        hit["text"].as_str().unwrap().chars().count() <= MAX_LINE_CHARS
            && hit["line_truncated"] == json!(false)
    }));
    assert_eq!(matches[0]["path"], json!("src/file-0000.txt"));
    assert_eq!(result["truncated"], json!(true));
}
