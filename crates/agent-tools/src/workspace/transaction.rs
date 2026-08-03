//! 可撤回单文件文本写入事务。

use crate::ToolError;
use crate::workspace::batch_journal;
use crate::workspace::journal_record::{self, JournalRecord, OriginalContents};
use crate::workspace::lock_set::WorkspaceLockSet;
use crate::workspace::mutation_guard;
use crate::workspace::revision::Revision;
use crate::workspace::target_path::{WorkspaceTarget, parse_workspace_target};
use crate::workspace::text_file::{
    io_error, read_current, replace_atomically, sync_parent, validate_text,
};
use std::fs;
use std::path::{Path, PathBuf};

/// 同一 workspace 内的可变更文件协调器。
pub(crate) struct WorkspaceTransactionCoordinator {
    pub(super) root: PathBuf,
    pub(super) locks: WorkspaceLockSet,
}

/// 成功写入后供调用方保存的撤回凭证。
#[derive(Debug)]
pub(crate) struct WorkspaceChange {
    pub(super) change_id: String,
    pub(super) before: Revision,
    pub(super) after: Revision,
}

impl WorkspaceChange {
    pub(crate) fn change_id(&self) -> &str {
        &self.change_id
    }

    pub(crate) fn before_revision(&self) -> &Revision {
        &self.before
    }

    pub(crate) fn after_revision(&self) -> &Revision {
        &self.after
    }
}

impl WorkspaceTransactionCoordinator {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Result<Self, ToolError> {
        let root = root.into().canonicalize().map_err(io_error)?;
        if !root.is_dir() {
            return Err(tool_err("bad_root", "workspace root 不是目录"));
        }
        Ok(Self {
            root,
            locks: WorkspaceLockSet::default(),
        })
    }

    /// 用 `expected` 比对当前 revision 后原子替换一个文本文件。
    pub(crate) fn write_text(
        &self,
        raw_target: &str,
        expected: &Revision,
        replacement: &str,
    ) -> Result<WorkspaceChange, ToolError> {
        validate_text(replacement.as_bytes())?;
        let target = self.parse_target(raw_target)?;
        let _guard = mutation_guard::acquire(
            &self.root,
            &self.locks,
            [journal_lock_target(), target.relative().to_path_buf()],
        )?;
        let target = self.parse_target(raw_target)?;
        let (original, current) = read_current(&target)?;
        if !expected.matches(&current) {
            return Err(conflict(expected, &current));
        }
        let after = Revision::for_contents(replacement.as_bytes());
        let record = journal_record::prepare(&self.root, &target, &current, &original, &after)?;
        replace_atomically(target.absolute(), replacement.as_bytes())?;
        journal_record::mark_committed(&self.root, &record)?;
        Ok(WorkspaceChange {
            change_id: record.change_id().to_owned(),
            before: current,
            after,
        })
    }

    /// 用 journal 的 preimage 恢复一个已提交变更；目标被外部改动时拒绝覆盖。
    pub(crate) fn revert(&self, change_id: &str) -> Result<Revision, ToolError> {
        let _journal_guard =
            mutation_guard::acquire(&self.root, &self.locks, [journal_lock_target()])?;
        if let Some(record) = batch_journal::load_committed(&self.root, change_id)? {
            return self.revert_batch(record);
        }
        let record = journal_record::load_committed(&self.root, change_id)?;
        let target = self.parse_target(record.target())?;
        let _target_guard = self.locks.acquire_many([target.relative().to_path_buf()]);
        revert_record(&target, &record)
    }

    /// 读取一个目标的存在状态与可用于下一次写入的 revision。
    pub(crate) fn inspect(&self, raw_target: &str) -> Result<(String, bool, Revision), ToolError> {
        let target = self.parse_target(raw_target)?;
        let _guard = self.locks.acquire_many([target.relative().to_path_buf()]);
        let target = self.parse_target(raw_target)?;
        let (original, revision) = read_current(&target)?;
        let path = target.relative().to_string_lossy().into_owned();
        Ok((
            path,
            matches!(original, OriginalContents::Present(_)),
            revision,
        ))
    }

    /// 读取一个可写文本文件的完整内容、存在状态与同一快照的 revision。标准
    /// `read_file` 用它把并发写入所需的 token 直接交给模型，而不用暴露额外的内部
    /// 前置工具；不存在的文件返回 `absent:v1`，可直接作为安全创建的前置条件。
    pub(crate) fn read_text(
        &self,
        raw_target: &str,
    ) -> Result<(String, bool, String, Revision), ToolError> {
        let target = self.parse_target(raw_target)?;
        let _guard = self.locks.acquire_many([target.relative().to_path_buf()]);
        let target = self.parse_target(raw_target)?;
        let (original, revision) = read_current(&target)?;
        let (exists, content) = match original {
            OriginalContents::Absent => (false, String::new()),
            OriginalContents::Present(contents) => (
                true,
                String::from_utf8(contents)
                    .map_err(|_| tool_err("not_text", "只支持 UTF-8 文本文件"))?,
            ),
        };
        let path = target.relative().to_string_lossy().into_owned();
        Ok((path, exists, content, revision))
    }

    pub(super) fn parse_target(&self, raw_target: &str) -> Result<WorkspaceTarget, ToolError> {
        let target = parse_workspace_target(&self.root, raw_target)?;
        if target
            .relative()
            .starts_with(Path::new(".agent/workspace-journal"))
        {
            return Err(tool_err(
                "reserved_path",
                "workspace journal 不能作为普通变更目标",
            ));
        }
        Ok(target)
    }
}

fn revert_record(target: &WorkspaceTarget, record: &JournalRecord) -> Result<Revision, ToolError> {
    let (_, current) = read_current(target)?;
    if !record.after().matches(&current) {
        return Err(conflict(record.after(), &current));
    }
    match record.original() {
        OriginalContents::Absent => {
            fs::remove_file(target.absolute()).map_err(io_error)?;
            sync_parent(target.absolute());
        }
        OriginalContents::Present(contents) => {
            validate_text(contents)?;
            replace_atomically(target.absolute(), contents)?;
        }
    }
    Ok(record.before().clone())
}

pub(super) fn journal_lock_target() -> PathBuf {
    PathBuf::from(".agent/workspace-journal")
}

pub(super) fn conflict(expected: &Revision, current: &Revision) -> ToolError {
    tool_err(
        "conflict",
        format!(
            "revision 不匹配：expected={}, current={}",
            expected.as_str(),
            current.as_str()
        ),
    )
}

pub(super) fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}
