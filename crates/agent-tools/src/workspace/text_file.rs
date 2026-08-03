//! 工作区普通 UTF-8 文本文件的受限 I/O 原语。

use crate::ToolError;
use crate::workspace::journal_record::OriginalContents;
use crate::workspace::revision::Revision;
use crate::workspace::target_path::WorkspaceTarget;
use crate::workspace::transaction::tool_err;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 单个文本文件允许的最大内容字节数，避免 journal 与内存读写失控。
pub(crate) const MAX_TEXT_FILE_BYTES: usize = 1_048_576;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 读取目标的 preimage 与基于内容的 revision；拒绝 symlink、目录和非文本文件。
pub(crate) fn read_current(
    target: &WorkspaceTarget,
) -> Result<(OriginalContents, Revision), ToolError> {
    match fs::symlink_metadata(target.absolute()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((OriginalContents::Absent, Revision::absent()))
        }
        Err(error) => Err(io_error(error)),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(tool_err("unsafe_path", "不能修改 symlink"))
        }
        Ok(metadata) if !metadata.is_file() => Err(tool_err("bad_input", "target 必须是普通文件")),
        Ok(metadata) if metadata.len() > MAX_TEXT_FILE_BYTES as u64 => {
            Err(tool_err("file_too_large", "target 超出 1 MiB 文本文件上限"))
        }
        Ok(_) => {
            let contents = read_no_follow(target.absolute())?;
            validate_text(&contents)?;
            Ok((
                OriginalContents::Present(contents.clone()),
                Revision::for_contents(&contents),
            ))
        }
    }
}

/// 通过同目录临时文件 + rename 替换目标，确保正常路径不会暴露半写内容。
pub(crate) fn replace_atomically(target: &Path, contents: &[u8]) -> Result<(), ToolError> {
    let parent = target
        .parent()
        .ok_or_else(|| tool_err("bad_input", "target 没有父目录"))?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(tool_err("unsafe_path", "target 父目录不能是 symlink"));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(tool_err("bad_input", "target 父级不是目录"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(tool_err("not_found", "target 父目录不存在"));
        }
        Err(error) => return Err(io_error(error)),
    }
    let temp = allocate_temp_path(parent)?;
    let result =
        write_temp(&temp, contents).and_then(|()| fs::rename(&temp, target).map_err(io_error));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    sync_parent(target);
    Ok(())
}

pub(crate) fn validate_text(contents: &[u8]) -> Result<(), ToolError> {
    if contents.len() > MAX_TEXT_FILE_BYTES {
        return Err(tool_err("file_too_large", "文本内容超出 1 MiB 上限"));
    }
    if contents.contains(&0) || std::str::from_utf8(contents).is_err() {
        return Err(tool_err("not_text", "只支持不含 NUL 的 UTF-8 文本文件"));
    }
    Ok(())
}

pub(crate) fn sync_parent(target: &Path) {
    if let Some(parent) = target.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
}

pub(crate) fn io_error(error: std::io::Error) -> ToolError {
    tool_err("io_error", format!("工作区文件操作失败：{error}"))
}

fn allocate_temp_path(parent: &Path) -> Result<PathBuf, ToolError> {
    for _ in 0..32 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".agent-write-{}-{sequence}.tmp",
            std::process::id()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(tool_err("io_error", "无法分配原子写入临时文件"))
}

fn write_temp(path: &Path, contents: &[u8]) -> Result<(), ToolError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(contents).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

#[cfg(unix)]
fn read_no_follow(path: &Path) -> Result<Vec<u8>, ToolError> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                tool_err("unsafe_path", "不能读取 symlink")
            } else {
                io_error(error)
            }
        })?;
    read_bounded(&mut file)
}

#[cfg(not(unix))]
fn read_no_follow(path: &Path) -> Result<Vec<u8>, ToolError> {
    let mut file = File::open(path).map_err(io_error)?;
    read_bounded(&mut file)
}

fn read_bounded(file: &mut File) -> Result<Vec<u8>, ToolError> {
    let mut contents = Vec::new();
    file.take(MAX_TEXT_FILE_BYTES as u64 + 1)
        .read_to_end(&mut contents)
        .map_err(io_error)?;
    if contents.len() > MAX_TEXT_FILE_BYTES {
        return Err(tool_err("file_too_large", "target 超出 1 MiB 文本文件上限"));
    }
    Ok(contents)
}
