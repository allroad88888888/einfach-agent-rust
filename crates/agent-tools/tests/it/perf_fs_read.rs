//! `read_file` 的确定性输出预算：分页结果必须只包含请求的行窗口，并带可用于
//! 并发写入的 revision；进入 prompt 前经 core 截断后仍受字节上限约束。这里刻意不量 wall-clock；当前 executor
//! 没有公开的 IO 计数器，测试只钉住可从 API 观察到的资源边界。

use agent_tools::ToolExecutor;
use serde_json::json;
use crate::support::TestRoot;

const LINE_COUNT: usize = 4_096;
const PAGE_OFFSET: usize = 2_049;
const PAGE_LIMIT: usize = 32;
const PAYLOAD_BYTES: usize = 64;

#[test]
fn paged_read_has_an_exact_line_and_prompt_byte_budget() {
    let root = TestRoot::new("perf-read-page");
    let lines: Vec<String> = (1..=LINE_COUNT)
        .map(|line| format!("line-{line:04}:{}", "x".repeat(PAYLOAD_BYTES)))
        .collect();
    root.write("large.txt", &lines.join("\n"));
    let exec = ToolExecutor::new(root.path()).unwrap();

    let page_result: serde_json::Value = serde_json::from_str(
        &exec
            .execute(
                "read_file",
                &json!({ "path": "large.txt", "offset": PAGE_OFFSET, "limit": PAGE_LIMIT }),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        page_result["revision"]
            .as_str()
            .unwrap()
            .starts_with("file:sha256:v1:")
    );
    let page = page_result["content"].as_str().unwrap();
    let expected = lines[PAGE_OFFSET - 1..PAGE_OFFSET - 1 + PAGE_LIMIT].join("\n");

    assert_eq!(page, expected, "分页不能泄露窗口外的内容");
    assert_eq!(page.lines().count(), PAGE_LIMIT);
    assert!(
        page.len() <= PAGE_LIMIT * ("line-0000:".len() + PAYLOAD_BYTES + 1),
        "limit={PAGE_LIMIT} 的返回字节数必须由单行最大长度线性限制"
    );
    assert_eq!(
        agent_core::truncate_tool_output(page, agent_core::DEFAULT_TOOL_OUTPUT_BYTES),
        page,
        "小分页不应消耗截断预算或产生截断标记"
    );

    let full: serde_json::Value = serde_json::from_str(
        &exec
            .execute("read_file", &json!({ "path": "large.txt" }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(full["revision"], page_result["revision"]);
    let full = full["content"].as_str().unwrap();
    assert!(full.len() > agent_core::DEFAULT_TOOL_OUTPUT_BYTES);
    let prompt_view = agent_core::truncate_tool_output(full, agent_core::DEFAULT_TOOL_OUTPUT_BYTES);
    assert!(prompt_view.len() < agent_core::DEFAULT_TOOL_OUTPUT_BYTES + 200);
    assert!(prompt_view.contains("输出被截断"));
}
