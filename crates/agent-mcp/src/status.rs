//! 装载后每个 server 的**可序列化状态**：谁连上了、谁没有、为什么。**这一件事**
//! ——只是数据形状，不做 IO（连接是 `loader` 的事）。
//!
//! 失败隔离把「一个 server 起不来」变成一条结构化状态，而不是 panic、不是吞掉
//! （docs/MCP.md §失败隔离；对齐 Claude Code 的 `/mcp`：failed server 不阻塞会话）。
//! 这份状态可序列化——server 的逻辑标识 + 可用性位进 atom/快照（红线 3：活句柄住
//! store 外，配置与可用性进 store）。

use serde::{Deserialize, Serialize};

/// 一个 server 装载后的可用性。三态穷尽：连上了 / 试了连不上 / 没试（暂不支持）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    /// 握手 + `tools/list` 成功，活句柄已登记进 `McpRegistry`。带这个 server 贡献了
    /// 几个工具（`/mcp` 展示用）。
    Connected { tool_count: usize },
    /// 尝试连接但失败：命令不存在 / 握手失败 / 超时 / `tools/list` 失败——带原因。
    /// **其余 server 照常，会话照常起**（失败隔离）。
    Unavailable { reason: String },
    /// 配置解析成功但 M6 不装载：远端 http/sse 传输未实现（形状留位）。带原因。
    Unsupported { reason: String },
}

impl Availability {
    pub fn is_connected(&self) -> bool {
        matches!(self, Availability::Connected { .. })
    }
}

/// 一个 server 的 id + 它装载后的可用性。`loader` 每个 server 产出一条，顺序 = 配置
/// 顺序。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub id: String,
    pub availability: Availability,
}

impl ServerStatus {
    pub fn connected(id: impl Into<String>, tool_count: usize) -> Self {
        Self {
            id: id.into(),
            availability: Availability::Connected { tool_count },
        }
    }

    pub fn unavailable(id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            availability: Availability::Unavailable {
                reason: reason.into(),
            },
        }
    }

    pub fn unsupported(id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            availability: Availability::Unsupported {
                reason: reason.into(),
            },
        }
    }

    pub fn is_connected(&self) -> bool {
        self.availability.is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrips_and_reports_connectedness() {
        let s = ServerStatus::unavailable("a", "命令不存在");
        assert!(!s.is_connected());
        let bytes = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<ServerStatus>(&bytes).unwrap(), s);

        assert!(ServerStatus::connected("b", 3).is_connected());
    }
}
