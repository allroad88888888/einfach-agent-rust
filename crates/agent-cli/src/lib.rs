//! `agent-cli` 的库面。**逻辑一行没搬**——`main.rs` 依然是唯一的可执行入口，
//! 这份 `lib.rs` 只是把 `mod` 声明从 `main.rs` 挪过来，换成 `pub mod`。
//!
//! 为什么要这一步：`crates/agent-cli/tests/` 下的集成测试要走完整的假 SSE
//! 服务器 + `Session` 链路（Ctrl-C、`/undo` 家族……），这必须是一条真的
//! `tests/*.rs`（不是塞进某个 `#[cfg(test)]` 模块），跟 `agent-runtime/tests/
//! cancel.rs` 同一种测试形状。但 `tests/*.rs` 编译成独立 crate，只看得见**库**
//! target 导出的 `pub` 项——一个只有 `main.rs`（bin-only）的 crate 没有库
//! target，`tests/` 下的文件连 `repl`/`undo` 模块都引用不到。加这份 `lib.rs`
//! 是 Rust 自己的规则逼出来的最小结构调整，不是这次改动顺手做的重构。
//!
//! 027：`turn` 模块（022 时代「取消轮截断消息列表」那招的家）退役——
//! `Session::begin_turn` 接过「一轮到下一轮」，`undo::after_cancelled_turn`
//! 接过「取消轮怎么处理」，两者都是会话层面的正牌答案，不再需要一个专门
//! 模块手写字段搬运。
pub mod ext_stats;
pub mod ext_stats_report;
pub mod mcp;
pub mod model_switch;
pub mod print;
pub mod provider;
pub mod repl;
pub mod session_path;
pub mod session_start;
pub mod undo;
pub mod vision;
