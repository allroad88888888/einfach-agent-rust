//! 031 独立测试 agent：红线 8 行为面（`bind` 默认 `127.0.0.1`，`AGENT_BIND`
//! 显式才准 `0.0.0.0`）。只用 `lib.rs` 里公开的纯函数
//! （`resolve_bind_ip`/`default_bind_ip`/`default_bind_addr`）+ 真起一个服务器
//! 验证监听地址的网络行为——不读 `src/bind.rs` 源码。

mod http_indep_support;

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use agent_server::{default_bind_ip, resolve_bind_ip};
use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::server_harness::{HarnessConfig, start_on};

/// `resolve_bind_ip` 是纯函数：不传 `AGENT_BIND` 的等价物（`None`）该解析成
/// loopback；显式给 `"0.0.0.0"` 才准出全零地址；非法输入是硬失败，不悄悄退回
/// 默认值。
#[test]
fn resolve_bind_ip_defaults_to_loopback_and_requires_explicit_opt_in_for_unspecified() {
    let default = resolve_bind_ip(None).expect("None 该有默认值");
    assert!(
        default.is_loopback(),
        "不给 AGENT_BIND 时该是 loopback，实际 {default}"
    );

    let explicit = resolve_bind_ip(Some("0.0.0.0")).expect("显式 0.0.0.0 该被接受");
    assert!(
        explicit.is_unspecified(),
        "显式给了才准是全零地址，实际 {explicit}"
    );

    let bad = resolve_bind_ip(Some("not-an-ip"));
    assert!(bad.is_err(), "非法输入该硬失败，不是悄悄退回默认值");
}

/// `default_bind_ip()`（薄封装，读真实环境变量）在这个测试进程没有设
/// `AGENT_BIND` 的前提下，也该是 loopback——跟 `resolve_bind_ip(None)` 的
/// 默认值同一个结论，只是走的是「读环境变量」这条腿。
#[test]
fn default_bind_ip_is_loopback_when_the_env_var_is_unset() {
    assert!(
        std::env::var("AGENT_BIND").is_err(),
        "这个断言的前提是当前进程没设 AGENT_BIND，否则这条测试本身就不成立"
    );
    let ip = default_bind_ip().expect("默认配置该总是能解析出一个地址");
    assert!(ip.is_loopback(), "默认该是 loopback，实际 {ip}");
}

/// 起一个真的服务器，绑在「默认配置解出来的 IP + 随机端口」上（不摸真实
/// `AGENT_BIND` 环境变量，只用它在没设时该有的默认值）——从 `127.0.0.1`
/// 能连上；换成本机一个非环回网卡地址（同一台机器，同一个端口）连接被拒，
/// 因为监听的 socket 根本没绑在那张网卡上。环境里如果拿不到非环回地址，
/// 退化成直接断言 `local_addr().ip().is_loopback()`。
#[tokio::test(flavor = "multi_thread")]
async fn a_server_started_with_default_config_only_listens_on_loopback() {
    let upstream = FakeUpstream::start(vec![Script::Text("hi".to_string())]);
    let default_ip = default_bind_ip().expect("默认地址该总是能解析");
    let bind_addr = SocketAddr::new(default_ip, 0);

    let server = start_on(bind_addr, upstream.endpoint(), HarnessConfig::default()).await;
    assert!(
        server.addr.ip().is_loopback(),
        "默认配置起的服务器该绑在 loopback 上，实际 {}",
        server.addr.ip()
    );

    // 从 127.0.0.1 能连上。
    let loopback_addr = SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        server.addr.port(),
    );
    let connected = TcpStream::connect_timeout(&loopback_addr, Duration::from_millis(500));
    assert!(connected.is_ok(), "从 127.0.0.1 该能连上：{connected:?}");

    // 从本机一个非环回网卡地址（同一台机器）连同一个端口，该被拒——监听
    // socket 没绑在那张网卡上，不是防火墙层面的事，是 bind 地址本身的事。
    match find_a_non_loopback_local_ip() {
        Some(ip) => {
            let other_addr = SocketAddr::new(ip, server.addr.port());
            let result = TcpStream::connect_timeout(&other_addr, Duration::from_millis(500));
            assert!(
                result.is_err(),
                "从非环回地址 {ip} 连同一个端口该被拒，实际 {result:?}"
            );
        }
        None => {
            eprintln!(
                "这台机器上找不到非环回本机地址，退化成只断言 local_addr().ip().is_loopback()（已经在上面断言过了）"
            );
        }
    }
}

/// 找一个本机的非环回 IPv4 地址（比如 en0 的局域网地址）用来验证「同一台机器
/// 上换一张网卡的地址连不上默认绑定的服务器」。找不到就返回 `None`，调用方
/// 负责降级断言。
fn find_a_non_loopback_local_ip() -> Option<std::net::IpAddr> {
    // 用一个 UDP "connect"（不真的发包）问内核「如果我要连出去，本地会用哪个
    // 地址」——这是拿本机非环回地址的标准手法，不需要解析 `ifconfig` 的输出。
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() { None } else { Some(ip) }
}
