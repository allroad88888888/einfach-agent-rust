//! 把 `apply_patch` 的声明式操作变成经过完整前置条件校验的文件替换计划。

use crate::ToolError;
use crate::workspace::journal_record::OriginalContents;
use crate::workspace::patch_input::{ContentGuard, PatchOperation};
use crate::workspace::revision::Revision;
use crate::workspace::target_path::WorkspaceTarget;
use crate::workspace::text_file::validate_text;
use crate::workspace::transaction::tool_err;

#[derive(Debug)]
pub(crate) struct ObservedFile {
    pub(crate) target: WorkspaceTarget,
    pub(crate) original: OriginalContents,
    pub(crate) revision: Revision,
}

#[derive(Debug)]
pub(crate) struct StagedFile {
    pub(crate) target: WorkspaceTarget,
    pub(crate) original: OriginalContents,
    pub(crate) before: Revision,
    pub(crate) after: Revision,
    pub(crate) replacement: Option<Vec<u8>>,
}

pub(crate) fn build(
    operations: &[PatchOperation],
    observed: Vec<ObservedFile>,
) -> Result<Vec<StagedFile>, ToolError> {
    operations
        .iter()
        .zip(observed)
        .map(|(operation, observed)| build_one(operation, observed))
        .collect()
}

fn build_one(operation: &PatchOperation, observed: ObservedFile) -> Result<StagedFile, ToolError> {
    match operation {
        PatchOperation::AddFile { content, .. } => add_file(observed, content),
        PatchOperation::DeleteFile { guard, .. } => delete_file(observed, guard),
        PatchOperation::Replace {
            old_text,
            new_text,
            expected_replacements,
            ..
        } => replace_text(observed, old_text, new_text, *expected_replacements),
        PatchOperation::OverwriteFile { content, guard, .. } => {
            overwrite_file(observed, content, guard)
        }
    }
}

fn add_file(observed: ObservedFile, content: &str) -> Result<StagedFile, ToolError> {
    if matches!(observed.original, OriginalContents::Present(_)) {
        return Err(tool_err("already_exists", "add_file 的目标文件已存在"));
    }
    stage(observed, Some(content.as_bytes().to_vec()))
}

fn delete_file(observed: ObservedFile, guard: &ContentGuard) -> Result<StagedFile, ToolError> {
    check_guard(&observed.original, guard)?;
    stage(observed, None)
}

fn overwrite_file(
    observed: ObservedFile,
    content: &str,
    guard: &ContentGuard,
) -> Result<StagedFile, ToolError> {
    check_guard(&observed.original, guard)?;
    stage(observed, Some(content.as_bytes().to_vec()))
}

fn replace_text(
    observed: ObservedFile,
    old_text: &str,
    new_text: &str,
    expected_replacements: usize,
) -> Result<StagedFile, ToolError> {
    let contents = present_contents(&observed.original)?;
    let text =
        std::str::from_utf8(contents).map_err(|_| tool_err("not_text", "只支持 UTF-8 文本"))?;
    let replacements = text.match_indices(old_text).count();
    if replacements != expected_replacements {
        return Err(tool_err(
            "conflict",
            format!("replace 预期替换 {expected_replacements} 处，当前只找到 {replacements} 处"),
        ));
    }
    let replacement = text.replace(old_text, new_text).into_bytes();
    stage(observed, Some(replacement))
}

fn check_guard(original: &OriginalContents, guard: &ContentGuard) -> Result<(), ToolError> {
    let contents = present_contents(original)?;
    let matches = match guard {
        ContentGuard::Exact(expected) => contents == expected.as_bytes(),
        ContentGuard::Sha256(expected) => Revision::for_contents(contents)
            .as_str()
            .strip_prefix("file:sha256:v1:")
            .is_some_and(|actual| actual == &expected["sha256:".len()..]),
    };
    if matches {
        Ok(())
    } else {
        Err(tool_err("conflict", "文件内容已变化，拒绝覆盖或删除"))
    }
}

fn present_contents(original: &OriginalContents) -> Result<&[u8], ToolError> {
    match original {
        OriginalContents::Present(contents) => Ok(contents),
        OriginalContents::Absent => Err(tool_err("not_found", "目标文件不存在")),
    }
}

fn stage(observed: ObservedFile, replacement: Option<Vec<u8>>) -> Result<StagedFile, ToolError> {
    if let Some(contents) = &replacement {
        validate_text(contents)?;
    }
    let after = replacement
        .as_deref()
        .map(Revision::for_contents)
        .unwrap_or_else(Revision::absent);
    Ok(StagedFile {
        target: observed.target,
        original: observed.original,
        before: observed.revision,
        after,
        replacement,
    })
}

#[cfg(test)]
#[path = "patch_plan_tests.rs"]
mod tests;
