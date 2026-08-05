//! 本 crate 集成测试的单一 harness：所有用例编进一个二进制。
//! 为什么合并：267 个单文件测试 = 267 个链接产物 + 267 次进程启动，
//! 两天就把 target 堆到 58GB/88 万文件（2026-08-05 诊断）。
//! 新增测试 = 在 tests/it/ 下建文件 + 在这里加一行 mod。

mod common;
mod everything_server_042;
mod handshake_translate_042;
mod initialize_041;
mod jsonrpc_codec_041;
mod jsonrpc_malformed_041;
mod list_tools_duplicate_074;
mod mcp_config_044;
mod mcp_loader_044;
mod registry_concurrency_070;
mod registry_not_in_snapshot_042;
mod tools_call_041;
mod tools_list_041;
mod translate_byte_identical_041;
mod translate_naming_041;
mod translate_reversibility_041;
