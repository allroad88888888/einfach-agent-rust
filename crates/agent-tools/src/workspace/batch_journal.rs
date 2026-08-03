//! 多文件原子操作的单个可撤回 journal。

use crate::ToolError;
use crate::workspace::journal_record::{self, OriginalContents};
use crate::workspace::journal_storage::{self, JournalStorage};
use crate::workspace::revision::Revision;
use crate::workspace::target_path::WorkspaceTarget;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

/// 批量变更中的一个文件前后状态。
pub(crate) struct BatchEntry {
    target: String,
    before: Revision,
    after: Revision,
    original: OriginalContents,
}

impl BatchEntry {
    pub(crate) fn new(
        target: &WorkspaceTarget,
        before: Revision,
        after: Revision,
        original: OriginalContents,
    ) -> Result<Self, ToolError> {
        let target = target
            .relative()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| tool_err("bad_input", "path 必须是有效 UTF-8"))?;
        Ok(Self {
            target,
            before,
            after,
            original,
        })
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn before(&self) -> &Revision {
        &self.before
    }

    pub(crate) fn after(&self) -> &Revision {
        &self.after
    }

    pub(crate) fn original(&self) -> &OriginalContents {
        &self.original
    }
}

/// 一次多文件变更的持久化记录。
pub(crate) struct BatchJournalRecord {
    change_id: String,
    entries: Vec<BatchEntry>,
}

impl BatchJournalRecord {
    pub(crate) fn change_id(&self) -> &str {
        &self.change_id
    }

    pub(crate) fn entries(&self) -> &[BatchEntry] {
        &self.entries
    }
}

/// 先持久化全部 preimage，再创建一个 prepared manifest。
pub(crate) fn prepare(
    root: &Path,
    entries: Vec<BatchEntry>,
) -> Result<BatchJournalRecord, ToolError> {
    validate_entries(&entries)?;
    let storage = JournalStorage::ensure(root)?;
    let change_id = storage.allocate_change_id()?;
    let manifest_entries = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| persist_entry(&storage, &change_id, index, entry))
        .collect::<Result<Vec<_>, _>>()?;
    let value = json!({
        "version": 2,
        "kind": "file_batch",
        "phase": "prepared",
        "change_id": change_id,
        "entries": manifest_entries,
    });
    storage.write_manifest(&change_id, &value)?;
    Ok(BatchJournalRecord { change_id, entries })
}

/// 在物理变更全部成功后提交 batch manifest。
pub(crate) fn mark_committed(root: &Path, record: &BatchJournalRecord) -> Result<(), ToolError> {
    set_phase(root, record.change_id(), "committed")
}

/// 把 batch 标为正在撤回；中断后必须人工修复而不是继续变更。
pub(crate) fn mark_reverting(root: &Path, record: &BatchJournalRecord) -> Result<(), ToolError> {
    set_phase(root, record.change_id(), "reverting")
}

/// 在全部 preimage 恢复后记录已撤回状态。
pub(crate) fn mark_reverted(root: &Path, record: &BatchJournalRecord) -> Result<(), ToolError> {
    set_phase(root, record.change_id(), "reverted")
}

/// 若 `change_id` 是 batch，读取其已提交记录；非 batch 返回 `None`。
pub(crate) fn load_committed(
    root: &Path,
    change_id: &str,
) -> Result<Option<BatchJournalRecord>, ToolError> {
    if !journal_storage::valid_change_id(change_id) {
        return Err(tool_err("bad_input", "change_id 格式非法"));
    }
    journal_record::assert_healthy(root)?;
    let storage = JournalStorage::existing(root)?
        .ok_or_else(|| tool_err("change_not_found", "找不到变更记录"))?;
    let value = storage.read_manifest(change_id)?;
    if value.get("kind").and_then(Value::as_str) != Some("file_batch") {
        return Ok(None);
    }
    match value.get("phase").and_then(Value::as_str) {
        Some("committed") => {}
        Some("reverted") => return Err(tool_err("change_already_reverted", "变更已经撤回")),
        _ => return Err(journal_storage::repair_needed("批量变更 journal 未提交")),
    }
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| journal_storage::repair_needed("批量变更 entries 字段损坏"))?;
    if entries.is_empty() || entries.len() > MAX_BATCH_ENTRIES {
        return Err(journal_storage::repair_needed(
            "批量变更文件数量超出允许范围",
        ));
    }
    let entries = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| load_entry(&storage, change_id, index, entry))
        .collect::<Result<Vec<_>, _>>()?;
    validate_entries(&entries)?;
    Ok(Some(BatchJournalRecord {
        change_id: change_id.to_owned(),
        entries,
    }))
}

fn persist_entry(
    storage: &JournalStorage,
    change_id: &str,
    index: usize,
    entry: &BatchEntry,
) -> Result<Value, ToolError> {
    let preimage = match entry.original() {
        OriginalContents::Absent => Value::Null,
        OriginalContents::Present(contents) => {
            let filename = preimage_filename(change_id, index);
            storage.write_new(&filename, contents)?;
            Value::String(filename)
        }
    };
    Ok(json!({
        "target": entry.target(),
        "before_revision": entry.before().as_str(),
        "after_revision": entry.after().as_str(),
        "preimage": preimage,
    }))
}

fn load_entry(
    storage: &JournalStorage,
    change_id: &str,
    index: usize,
    value: &Value,
) -> Result<BatchEntry, ToolError> {
    let target = required_string(value, "target")?.to_owned();
    let before = Revision::from_token(required_string(value, "before_revision")?)?;
    let after = Revision::from_token(required_string(value, "after_revision")?)?;
    let original = match value.get("preimage") {
        Some(Value::Null) => OriginalContents::Absent,
        Some(Value::String(filename)) if filename == &preimage_filename(change_id, index) => {
            OriginalContents::Present(storage.read_preimage(filename)?)
        }
        _ => {
            return Err(journal_storage::repair_needed("批量变更 preimage 字段损坏"));
        }
    };
    Ok(BatchEntry {
        target,
        before,
        after,
        original,
    })
}

fn set_phase(root: &Path, change_id: &str, phase: &str) -> Result<(), ToolError> {
    let storage = JournalStorage::ensure(root)?;
    let mut value = storage.read_manifest(change_id)?;
    if value.get("kind").and_then(Value::as_str) != Some("file_batch") {
        return Err(journal_storage::repair_needed("批量变更 manifest 类型损坏"));
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| journal_storage::repair_needed("workspace journal manifest 不是对象"))?;
    object.insert("phase".to_owned(), Value::String(phase.to_owned()));
    storage.write_manifest(change_id, &value)
}

pub(crate) const MAX_BATCH_ENTRIES: usize = 16;

fn validate_entries(entries: &[BatchEntry]) -> Result<(), ToolError> {
    if entries.is_empty() || entries.len() > MAX_BATCH_ENTRIES {
        return Err(tool_err(
            "bad_input",
            format!("批量文件操作最多允许 {MAX_BATCH_ENTRIES} 个不同目标"),
        ));
    }
    let paths: BTreeSet<_> = entries.iter().map(|entry| entry.target()).collect();
    if paths.len() != entries.len() {
        return Err(tool_err("bad_input", "source 与 destination 必须不同"));
    }
    Ok(())
}

fn preimage_filename(change_id: &str, index: usize) -> String {
    format!("{change_id}-{index}.before")
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| journal_storage::repair_needed(format!("批量变更缺少 {key}")))
}

fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}
