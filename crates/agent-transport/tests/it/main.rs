//! 本 crate 集成测试的单一 harness：所有用例编进一个二进制。
//! 为什么合并：267 个单文件测试 = 267 个链接产物 + 267 次进程启动，
//! 两天就把 target 堆到 58GB/88 万文件（2026-08-05 诊断）。
//! 新增测试 = 在 tests/it/ 下建文件 + 在这里加一行 mod。

mod fake_sse;
mod file_upload_failures;
mod file_upload_success;
mod post_json;
