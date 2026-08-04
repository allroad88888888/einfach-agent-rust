//! host 可用性门：一个传输种类在一个 host 上跑不跑得起来。**这一件事**——不连接、
//! 不解析配置，只表达能力门（docs/MCP.md §「host 能力差异」）。
//!
//! 为什么现在就写死形状：stdio 只有 server/桌面 host 有子进程；浏览器 host 没有子进程、
//! 只能走 http。M6 只做 stdio，所以浏览器 host 上没有任何 MCP server——门要能表达
//! **「这个源在这个 host 上不可用」**，而不是假装它存在、到调用时才失败。等 http 传输
//! 的延后 issue 来了，浏览器 host 长出远端 server，这个门不用改破坏性接口。

use serde::{Deserialize, Serialize};

/// 一个 server 要跑起来所需的传输种类。`config::ServerConfig::transport_kind` 产出它。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TransportKind {
    /// spawn 子进程 + newline-delimited JSON-RPC（M6 实做）。
    Stdio,
    /// http/sse 远端（M6 只解析留位，传输延后）。
    Remote,
}

/// 运行时宿主形态。决定哪些传输在这里物理可行——不是「实现了没」（那是 loader 的
/// 判断），是「这个 host 有没有这个能力」。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Host {
    /// CLI / 独立 server / 桌面：能 spawn 子进程，也能发 http。M6 的 CLI 就是它。
    Server,
    /// 浏览器：没有子进程，只能走 http（远端）。M6 不落地，形状留位。
    Browser,
}

impl Host {
    /// 这个 host 支不支持这种传输。**server host stdio 恒可用**（M6 CLI 走这条）；
    /// 浏览器 host 的 stdio 不可用（没有子进程）——门在这里就能表达不可用，不是到
    /// 调用才失败。
    ///
    /// 注意「host 支持」≠「M6 已实现」：server host 也支持远端（能发 http），但 M6
    /// 没实现 http 传输——那是 `loader` 的「暂不支持」判断，不是 host 能力问题。
    pub fn supports(self, kind: TransportKind) -> bool {
        match (self, kind) {
            (Host::Server, _) => true,
            (Host::Browser, TransportKind::Stdio) => false,
            (Host::Browser, TransportKind::Remote) => true,
        }
    }

    /// 诊断/`/mcp` 展示用的稳定短名。
    pub fn label(self) -> &'static str {
        match self {
            Host::Server => "server",
            Host::Browser => "browser",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 门的穷举：server host 两种传输都支持；浏览器只支持远端。
    #[test]
    fn supports_matrix_is_exhaustive() {
        assert!(Host::Server.supports(TransportKind::Stdio));
        assert!(Host::Server.supports(TransportKind::Remote));
        assert!(!Host::Browser.supports(TransportKind::Stdio));
        assert!(Host::Browser.supports(TransportKind::Remote));
    }
}
