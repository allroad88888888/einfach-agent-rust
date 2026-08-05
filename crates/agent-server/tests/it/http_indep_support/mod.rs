//! 031 独立测试 agent 自己的 HTTP 假浏览器 + 假上游夹具集合。不看实现方
//! `crates/agent-server/src/http/` 的源码、不看 `tests/support/http_client.rs`
//! /`tests/support/http_server.rs`——每个子模块都是照协议规格（issue 031 全文 +
//! 实测出的 wire 字节）从零手写的。
#![allow(dead_code)]

pub mod chunked;
pub mod fake_upstream;
pub mod raw_http;
pub mod server_harness;
pub mod sse_client;
pub mod wire;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 每个用例一个独立临时目录，不清理（跟既有 crate 内其它测试的取舍一致）。
pub fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-server-http-indep-{name}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
