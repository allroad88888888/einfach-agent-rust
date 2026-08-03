//! `apply_patch` 的批量事务提交与失败恢复。

use crate::ToolError;
use crate::workspace::batch_journal::{self, BatchEntry, BatchJournalRecord};
use crate::workspace::file_ops::{remove_file_atomically, restore_batch_preimages};
use crate::workspace::mutation_guard;
use crate::workspace::patch_input::PatchInput;
use crate::workspace::patch_plan::{self, ObservedFile, StagedFile};
use crate::workspace::text_file::{read_current, replace_atomically};
use crate::workspace::transaction::{
    WorkspaceTransactionCoordinator, journal_lock_target, tool_err,
};

#[derive(Debug)]
pub(crate) struct PatchResult {
    pub(crate) change_id: Option<String>,
    pub(crate) changed_files: Vec<String>,
    pub(crate) dry_run: bool,
}

impl WorkspaceTransactionCoordinator {
    /// 校验全部操作后一次性提交；写入失败时恢复所有已经变更的 preimage。
    pub(crate) fn apply_patch(&self, input: &PatchInput) -> Result<PatchResult, ToolError> {
        let targets = self.patch_targets(input)?;
        let _guard = mutation_guard::acquire(
            &self.root,
            &self.locks,
            std::iter::once(journal_lock_target())
                .chain(targets.iter().map(|target| target.relative().to_path_buf())),
        )?;
        let observed = self.observe_patch_targets(input)?;
        let staged = patch_plan::build(&input.operations, observed)?;
        let changed: Vec<_> = staged
            .into_iter()
            .filter(|file| !file.before.matches(&file.after))
            .collect();
        let changed_files = changed
            .iter()
            .map(|file| file.target.relative().to_string_lossy().into_owned())
            .collect();
        if input.dry_run || changed.is_empty() {
            return Ok(PatchResult {
                change_id: None,
                changed_files,
                dry_run: input.dry_run,
            });
        }
        self.commit_patch(changed, changed_files)
    }

    fn patch_targets(
        &self,
        input: &PatchInput,
    ) -> Result<Vec<crate::workspace::target_path::WorkspaceTarget>, ToolError> {
        input
            .operations
            .iter()
            .map(|operation| self.parse_target(operation.path()))
            .collect()
    }

    fn observe_patch_targets(&self, input: &PatchInput) -> Result<Vec<ObservedFile>, ToolError> {
        self.patch_targets(input)?
            .into_iter()
            .map(|target| {
                let (original, revision) = read_current(&target)?;
                Ok(ObservedFile {
                    target,
                    original,
                    revision,
                })
            })
            .collect()
    }

    fn commit_patch(
        &self,
        changed: Vec<StagedFile>,
        changed_files: Vec<String>,
    ) -> Result<PatchResult, ToolError> {
        let entries = changed
            .iter()
            .map(|file| {
                BatchEntry::new(
                    &file.target,
                    file.before.clone(),
                    file.after.clone(),
                    file.original.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let record = batch_journal::prepare(&self.root, entries)?;
        if let Err(error) = apply_staged(&changed) {
            return self.abort_patch(&record, &changed, error);
        }
        if let Err(error) = batch_journal::mark_committed(&self.root, &record) {
            return Err(tool_err(
                "journal_needs_repair",
                format!(
                    "apply_patch 已落盘但无法提交 workspace journal；后续变更已 fail-closed：{}",
                    error.message
                ),
            ));
        }
        Ok(PatchResult {
            change_id: Some(record.change_id().to_owned()),
            changed_files,
            dry_run: false,
        })
    }

    fn abort_patch(
        &self,
        record: &BatchJournalRecord,
        changed: &[StagedFile],
        cause: ToolError,
    ) -> Result<PatchResult, ToolError> {
        let targets = changed
            .iter()
            .map(|file| file.target.clone())
            .collect::<Vec<_>>();
        match restore_batch_preimages(&targets, record)
            .and_then(|()| batch_journal::mark_reverted(&self.root, record))
        {
            Ok(()) => Err(tool_err(
                "patch_rolled_back",
                format!("apply_patch 未完成，已恢复原状态：{}", cause.message),
            )),
            Err(rollback_error) => Err(tool_err(
                "journal_needs_repair",
                format!(
                    "apply_patch 未完成且自动恢复失败；workspace journal 已保留以阻止后续写入：operation={}, rollback={}",
                    cause.message, rollback_error.message
                ),
            )),
        }
    }
}

fn apply_staged(changed: &[StagedFile]) -> Result<(), ToolError> {
    for file in changed {
        match &file.replacement {
            Some(contents) => replace_atomically(file.target.absolute(), contents)?,
            None => remove_file_atomically(&file.target)?,
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "patch_transaction_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "patch_transaction_perf_tests.rs"]
mod perf_tests;
