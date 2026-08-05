//! 本 crate 集成测试的单一 harness：所有用例编进一个二进制。
//! 为什么合并：267 个单文件测试 = 267 个链接产物 + 267 次进程启动，
//! 两天就把 target 堆到 58GB/88 万文件（2026-08-05 诊断）。
//! 新增测试 = 在 tests/it/ 下建文件 + 在这里加一行 mod。

mod classify_errors;
mod decode_response;
mod drift_predicted_cache;
mod encode_determinism;
mod glm_adapter;
mod image_provider_fallback;
mod intent_translation;
mod invariants_meta;
mod kimi_adapter;
mod late_tools;
mod skill_indep_late_system_placement;
mod stream_tool_args;
mod stream_usage_and_text;
mod support;
mod three_providers;
