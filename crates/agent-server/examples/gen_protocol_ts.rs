//! issue 032 的「再生成命令」：`cargo run -p agent-server --features ts --example
//! gen_protocol_ts`。把 `packages/protocol/src/generated/` 与
//! `packages/protocol/fixtures/events.json` 整个重新写一遍——跟
//! `agent_server::ts_protocol` 的一致性测试用的是**同一份**导出代码
//! （[`agent_server::ts_protocol::export_protocol_types`] /
//! [`agent_server::ts_protocol::write_fixtures`]），不是另开一套：改协议之后先跑
//! 这个命令，再跑 `cargo test -p agent-server --features ts` 确认生成物跟 Rust
//! 源同步。
//!
//! 只在 `ts` feature 后面存在（`Cargo.toml` 的 `required-features`），不开这个
//! feature 编不出这个二进制。

use agent_server::ts_protocol::{export_protocol_types, fixtures_path, generated_dir, write_fixtures};

fn main() {
    let generated = generated_dir();
    export_protocol_types(&generated).unwrap_or_else(|e| {
        panic!("导出协议 TS 类型到 {} 失败：{e}", generated.display());
    });
    println!("生成 {}", generated.display());

    let fixtures = fixtures_path();
    write_fixtures(&fixtures).unwrap_or_else(|e| {
        panic!("写 fixtures 到 {} 失败：{e}", fixtures.display());
    });
    println!("生成 {}", fixtures.display());
}
