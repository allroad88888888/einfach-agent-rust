//! 本 crate 集成测试的单一 harness：所有用例编进一个二进制。
//! 为什么合并：267 个单文件测试 = 267 个链接产物 + 267 次进程启动，
//! 两天就把 target 堆到 58GB/88 万文件（2026-08-05 诊断）。
//! 新增测试 = 在 tests/it/ 下建文件 + 在这里加一行 mod。

mod error_codes;
mod find_test_lint_commands;
mod find_test_lint_commands_schema;
mod fs_list_happy;
mod fs_read_happy;
mod fs_rg_search;
mod fs_search_files;
mod path_jail;
mod perf_find_test_lint_commands;
mod perf_fs_list;
mod perf_fs_read;
mod perf_interaction_specs;
mod perf_rg_search;
mod perf_search_files;
mod perf_shell_exec;
mod perf_workspace_tools;
mod shell_exec_happy;
mod shell_exec_status;
mod shell_input_validation;
mod shell_orphan_cleanup;
mod shell_spec_declaration;
mod shell_timeout;
mod shell_undo_barrier;
mod standard_workspace_tools;
mod support;
mod tool_schema_contract;
mod tool_table_stability;
mod truncation;
mod workspace_schema_contract;
mod workspace_tools;
mod workspace_write_concurrency;
