//! 工作区文件变更的私有基础能力边界。

pub(crate) mod batch_journal;
pub(crate) mod file_ops;
pub(crate) mod journal_record;
pub(crate) mod journal_storage;
pub(crate) mod lock_set;
pub(crate) mod mutation_guard;
pub(crate) mod patch_input;
pub(crate) mod patch_plan;
pub(crate) mod patch_transaction;
pub(crate) mod process_lock;
pub(crate) mod revision;
pub(crate) mod target_path;
pub(crate) mod text_file;
pub(crate) mod tool_adapter;
pub(crate) mod transaction;

#[cfg(test)]
mod file_operations_perf_tests;

#[cfg(test)]
mod file_operations_tests;

#[cfg(test)]
mod tool_adapter_tests;

#[cfg(test)]
mod transaction_tests;
