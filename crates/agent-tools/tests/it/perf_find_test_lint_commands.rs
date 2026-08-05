//! 命令发现的固定资源边界：manifest 数、摘录字节与响应字节均由工具侧硬限制。

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use crate::support::TestRoot;

const MANIFEST_COUNT: usize = 16;
const MANIFEST_BYTES: usize = 5_000;
const MAX_RESPONSE_BYTES: usize = 24 * 1024;

#[test]
fn fixed_large_manifest_set_has_deterministic_excerpt_and_response_budgets() {
    let root = TestRoot::new("perf-command-discovery-budgets");
    for index in 0..MANIFEST_COUNT {
        root.write(
            &format!("packages/pkg-{index:02}/pyproject.toml"),
            &format!(
                "[tool.pytest.ini_options]\npad = \"{}\"\n",
                "x".repeat(MANIFEST_BYTES)
            ),
        );
    }
    let executor = ToolExecutor::new(root.path()).unwrap();
    let first = executor
        .execute("srv:workspace/find_test_lint_commands", &json!({}))
        .unwrap();
    let second = executor
        .execute("srv:workspace/find_test_lint_commands", &json!({}))
        .unwrap();
    assert_eq!(first, second, "固定文件集的发现结果必须逐字节稳定");
    assert!(first.len() <= MAX_RESPONSE_BYTES);

    let result: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(result["manifests"].as_array().unwrap().len(), 10);
    assert!(result["commands"].as_array().unwrap().len() <= 32);
    assert!(result["truncated"].as_bool().unwrap());
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("excerpt budget"))
    );
}
