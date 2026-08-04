//! `/mcp` 渲染（045）：把 MCP 装载后每个 server 的可用性 + 它暴露的工具名渲染成
//! 缩进文本，对齐 Claude Code 的 `/mcp`。**纯函数**——跟 `agent_tree` 一样只是一个
//! 格式化器，数据来自 [`crate::mcp::McpStatus`]（装载期的可序列化快照，含起不来的
//! server 的原因，跟活 registry 无关）。
//!
//! 分组按 server id 前缀（`mcp:<id>/`）——名字里已带 server id 消歧，两个 server 的
//! 同名工具（`mcp:a/x` vs `mcp:b/x`）各归各段。server 与工具的顺序都由 `McpStatus`
//! 里排好（server 按 id、工具按 `tools/list`），这里原样渲染，不重排。

use std::fmt::Write;

use agent_mcp::Availability;

use crate::mcp::McpStatus;

/// 渲染 `/mcp` 的整段文本。没配任何 server → 一句提示怎么配。
pub fn render_mcp_status(status: &McpStatus) -> String {
    if status.servers.is_empty() {
        return "（没有配置任何 MCP server。把 .mcp.json 放进启动目录，或用 --mcp-config <path> 指定。）"
            .to_string();
    }
    let mut out = String::from("MCP servers:");
    for server in &status.servers {
        let (state, detail) = describe(&server.availability);
        let _ = write!(out, "\n  {}  {state}", server.id);
        if let Some(detail) = detail {
            let _ = write!(out, "：{detail}");
        }
        let prefix = format!("mcp:{}/", server.id);
        for name in status.tool_names.iter().filter(|n| n.starts_with(&prefix)) {
            let _ = write!(out, "\n    {name}");
        }
    }
    out
}

/// 三态可用性 → （状态词, 可选细节）。connected 带工具数，其余带原因。
fn describe(availability: &Availability) -> (&'static str, Option<String>) {
    match availability {
        Availability::Connected { tool_count } => ("connected", Some(format!("{tool_count} 个工具"))),
        Availability::Unavailable { reason } => ("unavailable", Some(reason.clone())),
        Availability::Unsupported { reason } => ("unsupported", Some(reason.clone())),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_mcp::ServerStatus;

    use super::*;

    #[test]
    fn empty_status_tells_how_to_configure() {
        let out = render_mcp_status(&McpStatus { servers: vec![], tool_names: vec![] });
        assert!(out.contains("没有配置"), "实际: {out}");
    }

    #[test]
    fn renders_all_three_states_with_reasons_and_tools() {
        let status = McpStatus {
            servers: vec![
                ServerStatus::connected("everything", 2),
                ServerStatus::unavailable("broken", "连接失败: 子进程起不来"),
                ServerStatus::unsupported("remote", "远端传输 http 在 M6 未实现"),
            ],
            tool_names: vec![Arc::from("mcp:everything/echo"), Arc::from("mcp:everything/add")],
        };
        let out = render_mcp_status(&status);
        assert!(out.contains("everything  connected"), "实际: {out}");
        assert!(out.contains("2 个工具"));
        assert!(out.contains("mcp:everything/echo"));
        assert!(out.contains("mcp:everything/add"));
        assert!(out.contains("broken  unavailable"));
        assert!(out.contains("子进程起不来"), "unavailable 带原因");
        assert!(out.contains("remote  unsupported"));
        assert!(out.contains("未实现"), "unsupported 带原因");
    }

    /// 工具按 server id 前缀分组：`mcp:a/x` 落 a 段、`mcp:b/y` 落 b 段，不串。
    #[test]
    fn tools_are_grouped_under_their_own_server() {
        let status = McpStatus {
            servers: vec![ServerStatus::connected("a", 1), ServerStatus::connected("b", 1)],
            tool_names: vec![Arc::from("mcp:a/x"), Arc::from("mcp:b/y")],
        };
        let out = render_mcp_status(&status);
        let a_seg = out.find("  a  ").expect("a 段");
        let b_seg = out.find("  b  ").expect("b 段");
        let ax = out.find("mcp:a/x").expect("mcp:a/x");
        let by = out.find("mcp:b/y").expect("mcp:b/y");
        assert!(a_seg < ax && ax < b_seg, "mcp:a/x 应在 a 段内: {out}");
        assert!(b_seg < by, "mcp:b/y 应在 b 段内: {out}");
    }
}
