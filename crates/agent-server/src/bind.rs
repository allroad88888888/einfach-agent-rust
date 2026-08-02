//! 红线 8：`bind` 默认 loopback，监听「全部网卡」的地址必须由 `AGENT_BIND`
//! 显式给出——这个文件的源码里**不允许出现那个地址的字面量**（`check-invariants.sh`
//! 的红线 8 检查在 `crates/agent-server/` 下 grep 它，警告级也不该触发，见
//! `docs/issues/031-http-sse.md` 的「红线 8」条目）。默认地址走
//! `std::net::Ipv4Addr::LOCALHOST` 常量，覆盖地址完全来自环境变量的值本身——
//! 这不是风格洁癖，是「default 安全、暴露是显式动作」这条设计（ARCHITECTURE.md
//! §部署形态）唯一站得住脚的实现方式：源码里但凡出现那个字面量，`AGENT_BIND`
//! 就不再是唯一能打开它的入口。
//!
//! # 为什么解析逻辑（[`resolve_bind_ip`]）跟真的读环境变量（[`default_bind_ip`]）拆成两层
//!
//! `std::env::set_var`/`remove_var` 在 2024 edition 是 `unsafe fn`（多线程下改
//! 进程级环境变量本身就不安全），而 `cargo test` 默认多线程并发跑各个测试函数
//! ——谁在测试里改一次 `AGENT_BIND`，同一个测试二进制里其它并发测试读到的值就
//! 可能是别人改过的。把「给定一个可选的原始字符串，算出绑定地址」写成不摸
//! 环境变量的纯函数，覆盖行为（含「给的字符串不是合法 IP」这种坏配置）就能在
//! 不碰真实环境变量的情况下被精确、并发安全地测到；真正读 `std::env::var` 的
//! [`default_bind_ip`] 反而只有一行，两头都简单。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const AGENT_BIND_ENV: &str = "AGENT_BIND";

/// `AGENT_BIND` 配错了（不是一个合法的 IP 地址）——硬失败,不是「悄悄退回
/// loopback」。用户显式设了这个变量就是想覆盖默认值,把一个打错的字符串当成
/// "没设"处理,是那种"配置错了却看起来在正常运行"的坑,本仓的诊断哲学见不得这个
/// （`docs/ARCHITECTURE.md`「服务端不做鉴权」一节旁的一贯态度：能报错就不要猜）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindConfigError(pub String);

impl std::fmt::Display for BindConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BindConfigError {}

/// 纯函数：给定 `AGENT_BIND` 的原始值（`None` = 环境变量没设,或者设了空
/// 字符串——空字符串跟没设是同一种「调用方没有明确表达覆盖意图」,不是「覆盖成
/// 空」），算出实际要绑的 IP。**不读环境变量**,理由见本文件模块文档。
pub fn resolve_bind_ip(raw: Option<&str>) -> Result<IpAddr, BindConfigError> {
    match raw {
        None | Some("") => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        Some(v) => v.parse::<IpAddr>().map_err(|_| {
            BindConfigError(format!("{AGENT_BIND_ENV}=\"{v}\" 不是合法的 IP 地址，检查拼写（这是硬失败，不会悄悄退回默认值）"))
        }),
    }
}

/// [`resolve_bind_ip`] 套上真实的 `std::env::var` 读取——`AgentServer` 默认走
/// 这条路径。
pub fn default_bind_ip() -> Result<IpAddr, BindConfigError> {
    resolve_bind_ip(std::env::var(AGENT_BIND_ENV).ok().as_deref())
}

/// [`default_bind_ip`] 拼上端口，给 `AgentServer::bind`/`serve` 用。
pub fn default_bind_addr(port: u16) -> Result<SocketAddr, BindConfigError> {
    Ok(SocketAddr::new(default_bind_ip()?, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_defaults_to_loopback() {
        assert_eq!(resolve_bind_ip(None).unwrap(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn empty_string_is_treated_like_unset() {
        assert_eq!(resolve_bind_ip(Some("")).unwrap(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn explicit_override_is_honored() {
        // 用常量算出字符串，源码里不写这个地址的字面量（本文件模块文档）。
        let wildcard = Ipv4Addr::UNSPECIFIED.to_string();
        assert_eq!(resolve_bind_ip(Some(&wildcard)).unwrap(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn explicit_override_can_be_a_non_loopback_host() {
        assert_eq!(resolve_bind_ip(Some("10.0.0.5")).unwrap(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)));
    }

    #[test]
    fn garbage_is_a_hard_error_not_a_silent_fallback() {
        let err = resolve_bind_ip(Some("not-an-ip")).unwrap_err();
        assert!(err.0.contains("not-an-ip"), "{err}");
    }

    #[test]
    fn default_addr_combines_loopback_with_the_given_port() {
        let addr = default_bind_addr(0).unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 0);
    }
}
