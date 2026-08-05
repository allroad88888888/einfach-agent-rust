//! 本 crate 集成测试的单一 harness：所有用例编进一个二进制。
//! 为什么合并：267 个单文件测试 = 267 个链接产物 + 267 次进程启动，
//! 两天就把 target 堆到 58GB/88 万文件（2026-08-05 诊断）。
//! 新增测试 = 在 tests/it/ 下建文件 + 在这里加一行 mod。

mod applier_batch_reverse_order;
mod applier_derived_reconnect;
mod applier_empty_entries_noop;
mod applier_evict_undo_recovery;
mod applier_redo_inverts_undo;
mod applier_resolve_call_coverage;
mod atom_basic;
mod atom_complex;
mod atom_reentrant;
mod common;
mod depend_primitive;
mod depth_and_panic;
mod family_twin;
mod history_derived_noop;
mod history_log;
mod history_record_set;
mod history_serde;
mod session_log_replay;
mod session_store_memory_full_chain;
mod snapshot_history_persistence;
mod snapshot_invalid_history;
mod snapshot_recovery_is_redo;
mod snapshot_roundtrip;
mod snapshot_schema_add;
mod snapshot_schema_remove;
mod snapshot_serde_key_is_string;
mod store_basic;
mod undo_redo_append_truncates;
mod undo_redo_barrier;
mod undo_redo_batch_chain;
mod undo_redo_nothing;
mod undo_redo_roundtrip;
mod undo_redo_turn_boundaries;
