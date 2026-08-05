//! 截断组合（issue 013 验收 3）：`fs/read` 的原始输出经
//! `agent_core::truncate_tool_output(&out, DEFAULT_TOOL_OUTPUT_BYTES)` 后，
//! 长度有界、带「输出被截断」可见标记、带原始字节数。
//!
//! executor 本身不截断（lib.rs 文档：截断在 core 边界做），所以这里先验证
//! executor 返回的是完整原文，再验证过 core 截断函数之后的结果。

mod support;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

#[test]
fn fs_read_output_is_bounded_after_core_truncation() {
    let root = TestRoot::new("truncate");
    // 单行、无换行，每个汉字 3 字节 × 20000 = 60000 字节，确保 > 32 KiB 上限。
    let big_line: String = "汉".repeat(20_000);
    let original_len = big_line.len();
    assert!(original_len > agent_core::DEFAULT_TOOL_OUTPUT_BYTES);
    root.write("big.txt", &big_line);

    let exec = ToolExecutor::new(root.path()).unwrap();
    let raw = exec
        .execute("srv:fs/read", &json!({ "path": "big.txt" }))
        .unwrap();
    assert_eq!(
        raw.len(),
        original_len,
        "executor 必须返回原始未截断输出，截断是 core 的事"
    );

    let truncated = agent_core::truncate_tool_output(&raw, agent_core::DEFAULT_TOOL_OUTPUT_BYTES);

    assert!(
        truncated.len() < agent_core::DEFAULT_TOOL_OUTPUT_BYTES + 200,
        "截断后长度必须有界（内容部分等于 limit，标记只占几十字节）"
    );
    assert!(truncated.contains("输出被截断"), "必须带可见截断标记");
    assert!(
        truncated.contains(&original_len.to_string()),
        "标记必须带原始字节数，让模型知道看到的是残缺的"
    );
}

#[test]
fn small_file_survives_truncation_untouched() {
    // 反向印证：没超限时截断函数是恒等操作，避免上面那条测试靠“反正都会截”蒙混过关。
    let root = TestRoot::new("truncate-small");
    root.write("small.txt", "short content\nsecond line");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let raw = exec
        .execute("srv:fs/read", &json!({ "path": "small.txt" }))
        .unwrap();
    let truncated = agent_core::truncate_tool_output(&raw, agent_core::DEFAULT_TOOL_OUTPUT_BYTES);

    assert_eq!(truncated, raw);
    assert!(!truncated.contains("输出被截断"));
}
