//! `srv:fs/list` 的确定性输出预算：一个大而固定的目录必须完整、按名字稳定排序；
//! 交给 core 的 prompt 视图必须受统一截断上限约束。不使用 wall-clock 断言。

use agent_tools::ToolExecutor;
use serde_json::json;
use crate::support::TestRoot;

const PAIRS: usize = 1_024;

#[test]
fn large_listing_is_sorted_and_its_prompt_view_is_byte_bounded() {
    let root = TestRoot::new("perf-list-sorted");
    let mut expected = Vec::with_capacity(PAIRS * 2);

    for index in 0..PAIRS {
        let name = format!("directory-{index:04}-xxxxxxxx");
        root.mkdir(&name);
        expected.push(format!("{name}/"));
    }
    for index in 0..PAIRS {
        let name = format!("file-{index:04}-xxxxxxxx.txt");
        root.write(&name, "x");
        expected.push(name);
    }

    let exec = ToolExecutor::new(root.path()).unwrap();
    let out = exec
        .execute("srv:fs/list", &json!({ "path": "." }))
        .unwrap();
    let expected = expected.join("\n");

    assert_eq!(out, expected, "目录枚举顺序必须与底层文件系统无关");
    assert_eq!(out.lines().count(), PAIRS * 2);
    assert!(
        out.len() > agent_core::DEFAULT_TOOL_OUTPUT_BYTES,
        "夹具必须足够大，才能覆盖统一 prompt 截断边界"
    );

    let prompt_view = agent_core::truncate_tool_output(&out, agent_core::DEFAULT_TOOL_OUTPUT_BYTES);
    assert!(prompt_view.len() < agent_core::DEFAULT_TOOL_OUTPUT_BYTES + 200);
    assert!(prompt_view.contains("输出被截断"));
}
