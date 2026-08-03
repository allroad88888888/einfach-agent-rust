//! 可撤回的单文件删除、复制与移动操作。

use crate::ToolError;
use crate::workspace::batch_journal::{self, BatchEntry, BatchJournalRecord};
use crate::workspace::journal_record::{self, OriginalContents};
use crate::workspace::mutation_guard;
use crate::workspace::revision::Revision;
use crate::workspace::target_path::WorkspaceTarget;
use crate::workspace::text_file::{io_error, read_current, replace_atomically, sync_parent};
use crate::workspace::transaction::{
    WorkspaceChange, WorkspaceTransactionCoordinator, conflict, journal_lock_target, tool_err,
};
use std::fs;

impl WorkspaceTransactionCoordinator {
    /// 删除 revision 仍匹配的文本文件，返回可撤回的 change。
    pub(crate) fn delete_file(
        &self,
        raw_target: &str,
        expected: &Revision,
    ) -> Result<WorkspaceChange, ToolError> {
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
        require_present(&original)?;
        let after = Revision::absent();
        let record = journal_record::prepare(&self.root, &target, &current, &original, &after)?;
        remove_file_atomically(&target)?;
        journal_record::mark_committed(&self.root, &record)?;
        Ok(change_from_single_record(&record))
    }

    /// 复制源文件到 destination；两个 revision 都必须来自最新 inspect。
    pub(crate) fn copy_file(
        &self,
        raw_source: &str,
        expected_source: &Revision,
        raw_destination: &str,
        expected_destination: &Revision,
    ) -> Result<WorkspaceChange, ToolError> {
        let (source, destination) = self.parse_pair(raw_source, raw_destination)?;
        let _guard = mutation_guard::acquire(
            &self.root,
            &self.locks,
            [
                journal_lock_target(),
                source.relative().to_path_buf(),
                destination.relative().to_path_buf(),
            ],
        )?;
        let (source, destination) = self.parse_pair(raw_source, raw_destination)?;
        let (source_original, source_current) = read_current(&source)?;
        let (destination_original, destination_current) = read_current(&destination)?;
        verify_expected(expected_source, &source_current)?;
        verify_expected(expected_destination, &destination_current)?;
        let contents = require_present(&source_original)?;
        let after = Revision::for_contents(contents);
        let record = journal_record::prepare(
            &self.root,
            &destination,
            &destination_current,
            &destination_original,
            &after,
        )?;
        replace_atomically(destination.absolute(), contents)?;
        journal_record::mark_committed(&self.root, &record)?;
        Ok(change_from_single_record(&record))
    }

    /// 移动源文件到 destination，作为一个包含两个 preimage 的可撤回 batch。
    pub(crate) fn move_file(
        &self,
        raw_source: &str,
        expected_source: &Revision,
        raw_destination: &str,
        expected_destination: &Revision,
    ) -> Result<WorkspaceChange, ToolError> {
        let (source, destination) = self.parse_pair(raw_source, raw_destination)?;
        let _guard = mutation_guard::acquire(
            &self.root,
            &self.locks,
            [
                journal_lock_target(),
                source.relative().to_path_buf(),
                destination.relative().to_path_buf(),
            ],
        )?;
        let (source, destination) = self.parse_pair(raw_source, raw_destination)?;
        let (source_original, source_current) = read_current(&source)?;
        let (destination_original, destination_current) = read_current(&destination)?;
        verify_expected(expected_source, &source_current)?;
        verify_expected(expected_destination, &destination_current)?;
        let contents = require_present(&source_original)?.to_owned();
        let source_after = Revision::absent();
        let destination_after = Revision::for_contents(&contents);
        let record = batch_journal::prepare(
            &self.root,
            vec![
                BatchEntry::new(
                    &source,
                    source_current.clone(),
                    source_after.clone(),
                    source_original,
                )?,
                BatchEntry::new(
                    &destination,
                    destination_current,
                    destination_after,
                    destination_original,
                )?,
            ],
        )?;
        if let Err(error) = replace_atomically(destination.absolute(), &contents)
            .and_then(|()| remove_file_atomically(&source))
        {
            return self.abort_move(&record, &[source, destination], error);
        }
        if let Err(error) = batch_journal::mark_committed(&self.root, &record) {
            return Err(journal_repair_after_move(error));
        }
        Ok(WorkspaceChange {
            change_id: record.change_id().to_owned(),
            before: source_current,
            after: source_after,
        })
    }

    pub(super) fn revert_batch(&self, record: BatchJournalRecord) -> Result<Revision, ToolError> {
        let targets = self.batch_targets(&record)?;
        let _targets_guard = self
            .locks
            .acquire_many(targets.iter().map(|target| target.relative().to_path_buf()));
        let targets = self.batch_targets(&record)?;
        for (target, entry) in targets.iter().zip(record.entries()) {
            let (_, current) = read_current(target)?;
            if !entry.after().matches(&current) {
                return Err(conflict(entry.after(), &current));
            }
        }
        batch_journal::mark_reverting(&self.root, &record)?;
        for (target, entry) in targets.iter().zip(record.entries()) {
            restore_original(target, entry.original())?;
        }
        batch_journal::mark_reverted(&self.root, &record)?;
        Ok(record.entries()[0].before().clone())
    }

    /// 在移动只完成一部分时恢复两个目标；无法确认恢复时保留 prepared journal。
    fn abort_move(
        &self,
        record: &BatchJournalRecord,
        targets: &[WorkspaceTarget],
        cause: ToolError,
    ) -> Result<WorkspaceChange, ToolError> {
        match restore_batch_preimages(targets, record)
            .and_then(|()| batch_journal::mark_reverted(&self.root, record))
        {
            Ok(()) => Err(tool_err(
                "move_rolled_back",
                format!("移动未完成，已恢复原状态：{}", cause.message),
            )),
            Err(rollback_error) => Err(tool_err(
                "journal_needs_repair",
                format!(
                    "移动未完成且自动恢复失败；workspace journal 已保留以阻止后续写入：operation={}, rollback={}",
                    cause.message, rollback_error.message
                ),
            )),
        }
    }

    fn parse_pair(
        &self,
        raw_source: &str,
        raw_destination: &str,
    ) -> Result<(WorkspaceTarget, WorkspaceTarget), ToolError> {
        let source = self.parse_target(raw_source)?;
        let destination = self.parse_target(raw_destination)?;
        if source.relative() == destination.relative() {
            return Err(tool_err("bad_input", "source 与 destination 必须不同"));
        }
        Ok((source, destination))
    }

    fn batch_targets(
        &self,
        record: &BatchJournalRecord,
    ) -> Result<Vec<WorkspaceTarget>, ToolError> {
        record
            .entries()
            .iter()
            .map(|entry| self.parse_target(entry.target()))
            .collect()
    }
}

fn change_from_single_record(record: &journal_record::JournalRecord) -> WorkspaceChange {
    WorkspaceChange {
        change_id: record.change_id().to_owned(),
        before: record.before().clone(),
        after: record.after().clone(),
    }
}

fn verify_expected(expected: &Revision, current: &Revision) -> Result<(), ToolError> {
    if expected.matches(current) {
        Ok(())
    } else {
        Err(conflict(expected, current))
    }
}

fn require_present(original: &OriginalContents) -> Result<&[u8], ToolError> {
    match original {
        OriginalContents::Present(contents) => Ok(contents),
        OriginalContents::Absent => Err(tool_err("not_found", "目标文件不存在")),
    }
}

pub(super) fn remove_file_atomically(target: &WorkspaceTarget) -> Result<(), ToolError> {
    fs::remove_file(target.absolute()).map_err(io_error)?;
    sync_parent(target.absolute());
    Ok(())
}

pub(super) fn restore_original(
    target: &WorkspaceTarget,
    original: &OriginalContents,
) -> Result<(), ToolError> {
    match original {
        OriginalContents::Absent => remove_file_atomically(target),
        OriginalContents::Present(contents) => replace_atomically(target.absolute(), contents),
    }
}

pub(super) fn restore_batch_preimages(
    targets: &[WorkspaceTarget],
    record: &BatchJournalRecord,
) -> Result<(), ToolError> {
    for (target, entry) in targets.iter().zip(record.entries()) {
        let (_, current) = read_current(target)?;
        if entry.before().matches(&current) {
            continue;
        }
        if !entry.after().matches(&current) {
            return Err(tool_err(
                "conflict",
                format!("批量变更中途失败后目标被外部改动：{}", entry.target()),
            ));
        }
    }
    for (target, entry) in targets.iter().zip(record.entries()) {
        let (_, current) = read_current(target)?;
        if entry.after().matches(&current) {
            restore_original(target, entry.original())?;
        }
    }
    Ok(())
}

fn journal_repair_after_move(error: ToolError) -> ToolError {
    tool_err(
        "journal_needs_repair",
        format!(
            "移动已落盘但无法提交 workspace journal；后续变更已 fail-closed：{}",
            error.message
        ),
    )
}
