//! 进程级运行日志出口。
//!
//! `AGENT_LOG_FORMAT=json` 输出 JSON；未设置或取其他值时输出紧凑人类可读文本。
//! `RUST_LOG` 控制过滤规则，未设置或规则无效时回退到 `info`。

use tracing_subscriber::EnvFilter;

const LOG_FORMAT_ENV: &str = "AGENT_LOG_FORMAT";

/// 安装本二进制唯一的 `tracing` subscriber。
///
/// 返回 `false` 表示已有全局 subscriber 或安装失败；调用方应继续提供服务，不能
/// 因诊断设施不可用而使进程 panic。
pub fn install() -> bool {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match std::env::var(LOG_FORMAT_ENV).as_deref() {
        Ok("json") => tracing_subscriber::fmt()
            .json()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .try_init()
            .is_ok(),
        _ => tracing_subscriber::fmt()
            .compact()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .try_init()
            .is_ok(),
    }
}
