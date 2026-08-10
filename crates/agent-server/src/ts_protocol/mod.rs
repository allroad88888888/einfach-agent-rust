//! issue 032：协议类型的 TS 生成与一致性检查。整个模块只在 `ts` feature 后面
//! 存在（agent-core 默认关、agent-server 打开它，见两边 `Cargo.toml` 注释）——
//! 正常 `cargo build`/`cargo test`（不开 feature）里，ts-rs 连编译都不参与。
//!
//! 两个子模块各管一件事：
//! - [`export`]：把 [`crate::Command`]/[`crate::Frame`] 连同它们递归的
//!   全部依赖类型导出成 `.ts` 文件（034：`Frame` 是 agent 归属信封，
//!   `SessionEvent` 是它的依赖，一并导出）
//! - [`fixtures`]：`SessionEvent` 每个变体各铸一个样本、包成 `Frame`，序列化进
//!   `fixtures.json`——serde↔TS 形状对齐的实检（TS 那边靠 `fixtures.test.ts`
//!   的 `satisfies`）
//!
//! 一致性检查本身（导出到临时目录、跟仓库里已经生成的 `packages/protocol/`
//! 逐字节比较）住在 [`consistency`] 里，只在 `#[cfg(test)]` 编译。
//!
//! 两个导出函数是 `pub`：`examples/gen_protocol_ts.rs`——032 要求的「再生成
//! 命令」——要调用**同一份**导出代码，不能另起一套（否则「重新生成」和「一致性
//! 检查用的生成」跑的不是一条路径，两边分叉是迟早的事）。这不是常规意义上的库
//! 公开面：整个 `ts_protocol` 模块连同它的 `pub` 都长在 `ts` feature 门后面，
//! 不开这个 feature 的调用方看不到它，也用不上它。

mod export;
mod fixtures;
mod fixtures_cast;

#[cfg(test)]
mod consistency;

pub use export::export_protocol_types;
#[cfg(test)]
pub(crate) use fixtures::sample_session_events;
pub use fixtures::write_fixtures;

use std::path::{Path, PathBuf};

/// `packages/protocol/src/generated/` 的绝对路径——「路径统一常量」（issue 032
/// 原话）落在这一个函数上，[`consistency`] 的测试和 `examples/gen_protocol_ts.rs`
/// 都只认它，不各自拼字符串。
pub fn generated_dir() -> PathBuf {
    workspace_root().join("packages/protocol/src/generated")
}

/// `packages/protocol/fixtures/events.json` 的绝对路径，跟 [`generated_dir`]
/// 同一个锚点、同一条理由。
pub fn fixtures_path() -> PathBuf {
    workspace_root().join("packages/protocol/fixtures/events.json")
}

/// 用 `CARGO_MANIFEST_DIR`（编译期常量，指向 `crates/agent-server`）往上跳两级
/// 到 workspace 根，不用相对 cwd 拼——`cargo test`/`cargo run --example` 的 cwd
/// 恰好都是 crate 目录，但「恰好」不是「保证」，`env!` 直接锚死更省心。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/agent-server 到 workspace 根总是两层 parent")
        .to_path_buf()
}
