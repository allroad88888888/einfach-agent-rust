//! 单文件变更的持久化 preimage 与 manifest。

use crate::ToolError;
use crate::workspace::journal_storage::{self, JournalStorage};
use crate::workspace::revision::Revision;
use crate::workspace::target_path::WorkspaceTarget;
use serde_json::{Value, json};
use std::path::Path;

/// 已写入 manifest 的单文件变更记录。
pub(crate) struct JournalRecord {
    change_id: String,
    target: String,
    before: Revision,
    after: Revision,
    original: OriginalContents,
}

/// 写入前目标的内容状态。
#[derive(Clone, Debug)]
pub(crate) enum OriginalContents {
    Absent,
    Present(Vec<u8>),
}

impl JournalRecord {
    pub(crate) fn change_id(&self) -> &str {
        &self.change_id
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

/// 只要发现未提交或损坏的 manifest，后续 mutation 必须 fail-closed。
pub(crate) fn assert_healthy(root: &Path) -> Result<(), ToolError> {
    let Some(storage) = JournalStorage::existing(root)? else {
        return Ok(());
    };
    for entry in storage.entries()? {
        let entry = entry.map_err(|error| journal_storage::repair_needed(error.to_string()))?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let record = storage.read_manifest_path(&entry.path())?;
            let phase = record.get("phase").and_then(Value::as_str);
            if !matches!(phase, Some("committed") | Some("reverted")) {
                return Err(journal_storage::repair_needed(
                    "发现未完成的 workspace journal",
                ));
            }
        }
    }
    Ok(())
}

/// 先持久化 preimage 与 prepared manifest；调用方随后才可以覆盖目标。
pub(crate) fn prepare(
    root: &Path,
    target: &WorkspaceTarget,
    before: &Revision,
    original: &OriginalContents,
    after: &Revision,
) -> Result<JournalRecord, ToolError> {
    let storage = JournalStorage::ensure(root)?;
    let change_id = storage.allocate_change_id()?;
    let target = target_to_string(target)?;
    let preimage = persist_preimage(&storage, &change_id, original, ".before")?;
    let value = json!({
        "version": 1,
        "phase": "prepared",
        "change_id": change_id,
        "target": target,
        "before_revision": before.as_str(),
        "after_revision": after.as_str(),
        "preimage": preimage,
    });
    storage.write_manifest(&change_id, &value)?;
    Ok(JournalRecord {
        change_id,
        target,
        before: before.clone(),
        after: after.clone(),
        original: match preimage {
            Some(filename) => OriginalContents::Present(storage.read_preimage(&filename)?),
            None => OriginalContents::Absent,
        },
    })
}

/// 将 manifest 切换为 committed；该写入失败时调用方必须报告需修复的 journal。
pub(crate) fn mark_committed(root: &Path, record: &JournalRecord) -> Result<(), ToolError> {
    let storage = JournalStorage::ensure(root)?;
    set_phase(&storage, record.change_id(), "committed")
}

/// 读取已经提交的变更记录，用于撤回。
pub(crate) fn load_committed(root: &Path, change_id: &str) -> Result<JournalRecord, ToolError> {
    if !journal_storage::valid_change_id(change_id) {
        return Err(tool_err("bad_input", "change_id 格式非法"));
    }
    assert_healthy(root)?;
    let storage = JournalStorage::existing(root)?
        .ok_or_else(|| tool_err("change_not_found", "找不到变更记录"))?;
    let value = storage.read_manifest(change_id)?;
    if value.get("kind").is_some() {
        return Err(tool_err("batch_change", "change_id 指向批量变更"));
    }
    if value.get("phase").and_then(Value::as_str) != Some("committed") {
        return Err(journal_storage::repair_needed(
            "workspace journal 记录未提交",
        ));
    }
    let target = required_string(&value, "target")?.to_owned();
    let before = Revision::from_token(required_string(&value, "before_revision")?)?;
    let after = Revision::from_token(required_string(&value, "after_revision")?)?;
    let expected_preimage = format!("{change_id}.before");
    let original = restore_manifest_preimage(&storage, value.get("preimage"), &expected_preimage)?;
    Ok(JournalRecord {
        change_id: change_id.to_owned(),
        target,
        before,
        after,
        original,
    })
}

fn target_to_string(target: &WorkspaceTarget) -> Result<String, ToolError> {
    target
        .relative()
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| tool_err("bad_input", "path 必须是有效 UTF-8"))
}

fn persist_preimage(
    storage: &JournalStorage,
    change_id: &str,
    original: &OriginalContents,
    suffix: &str,
) -> Result<Option<String>, ToolError> {
    match original {
        OriginalContents::Absent => Ok(None),
        OriginalContents::Present(contents) => {
            let filename = format!("{change_id}{suffix}");
            storage.write_new(&filename, contents)?;
            Ok(Some(filename))
        }
    }
}

fn restore_manifest_preimage(
    storage: &JournalStorage,
    value: Option<&Value>,
    expected_filename: &str,
) -> Result<OriginalContents, ToolError> {
    match value {
        Some(Value::Null) | None => Ok(OriginalContents::Absent),
        Some(Value::String(filename)) if filename == expected_filename => {
            Ok(OriginalContents::Present(storage.read_preimage(filename)?))
        }
        _ => Err(journal_storage::repair_needed(
            "workspace journal preimage 字段损坏",
        )),
    }
}

fn set_phase(storage: &JournalStorage, change_id: &str, phase: &str) -> Result<(), ToolError> {
    let mut value = storage.read_manifest(change_id)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| journal_storage::repair_needed("workspace journal manifest 不是对象"))?;
    object.insert("phase".to_owned(), Value::String(phase.to_owned()));
    storage.write_manifest(change_id, &value)
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| journal_storage::repair_needed(format!("workspace journal 缺少 {key}")))
}

fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}
