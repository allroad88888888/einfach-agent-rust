//! 结构性证明：红线 3 说「MCP 子进程句柄放 store 外面的 runtime registry，atom
//! 里只放可序列化句柄」。`McpRegistry`/`McpClient` 本身不 derive `Serialize`，
//! 但光看类型定义说服力有限（谁都能读代码确认一次，改天也可能被悄悄破坏）。
//! 真正钉死这条红线的是**依赖方向**：`agent-core`（`AgentValue`/`Session`/快照
//! 全部住在这个 crate）压根不依赖 `agent-mcp`——类型层面就够不着
//! `McpClient`/`McpRegistry`，不是「没写进去」而是「写不进去」。`agent-store`
//! （落盘那一层）同理。
//!
//! 这个测试钉住依赖方向本身，防止将来有人为了图方便让 `agent-core`/
//! `agent-store` 反向依赖 `agent-mcp`——一旦那条依赖成立，红线 3 就只剩注释在
//! 挡，没有编译期保证了。

use std::fs;
use std::path::Path;

#[test]
fn agent_core_cargo_toml_does_not_depend_on_agent_mcp() {
    assert_no_agent_mcp_dependency("../agent-core/Cargo.toml");
}

#[test]
fn agent_store_cargo_toml_does_not_depend_on_agent_mcp() {
    assert_no_agent_mcp_dependency("../agent-store/Cargo.toml");
}

fn assert_no_agent_mcp_dependency(relative_manifest: &str) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join(relative_manifest);
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到 {}: {e}（workspace 布局变了？）", path.display()));

    assert!(
        !contents.contains("agent-mcp"),
        "{} 不该依赖 agent-mcp——红线 3 靠的正是这个方向：MCP 的活句柄\
         （Child/pipe/reader 线程）住在 agent-mcp::McpRegistry 里，这个 crate 的\
         原子/快照类型层面够不着它们；不是靠约定，是靠这条依赖方向从根上不成立。",
        path.display()
    );
}
