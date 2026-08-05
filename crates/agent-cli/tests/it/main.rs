//! 本 crate 集成测试的单一 harness：所有用例编进一个二进制。
//! 为什么合并：267 个单文件测试 = 267 个链接产物 + 267 次进程启动，
//! 两天就把 target 堆到 58GB/88 万文件（2026-08-05 诊断）。
//! 新增测试 = 在 tests/it/ 下建文件 + 在这里加一行 mod。

mod cancel_after_shell_kept;
mod cancel_flow;
mod indep_cancel_erase;
mod indep_corrupt_session;
mod indep_restart_continue;
mod indep_shell_barrier;
mod indep_support;
mod indep_undo_roundtrip;
mod indep_unresolved_tool_recovery;
mod shell_undo_flow;
mod support;
