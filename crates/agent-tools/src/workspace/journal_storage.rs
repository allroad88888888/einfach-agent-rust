//! workspace journal 的受保护存储布局。

use crate::ToolError;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 已确认是普通目录的 workspace journal `changes` 目录。
pub(crate) struct JournalStorage {
    changes: PathBuf,
}

impl JournalStorage {
    /// 创建 journal 路径；已有 symlink 或异常节点一律拒绝。
    pub(crate) fn ensure(root: &Path) -> Result<Self, ToolError> {
        let agent = root.join(".agent");
        ensure_plain_dir(&agent)?;
        let journal = agent.join("workspace-journal");
        ensure_plain_dir(&journal)?;
        let changes = journal.join("changes");
        ensure_plain_dir(&changes)?;
        Ok(Self { changes })
    }

    /// 打开既有 journal；尚未发生过变更时返回 `None`。
    pub(crate) fn existing(root: &Path) -> Result<Option<Self>, ToolError> {
        let agent = root.join(".agent");
        if !plain_dir_if_exists(&agent)? {
            return Ok(None);
        }
        let journal = agent.join("workspace-journal");
        if !plain_dir_if_exists(&journal)? {
            return Ok(None);
        }
        let changes = journal.join("changes");
        if plain_dir_if_exists(&changes)? {
            Ok(Some(Self { changes }))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn entries(&self) -> Result<fs::ReadDir, ToolError> {
        fs::read_dir(&self.changes).map_err(journal_io)
    }

    pub(crate) fn allocate_change_id(&self) -> Result<String, ToolError> {
        for _ in 0..32 {
            let sequence = JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| repair_needed(format!("系统时间不可用：{error}")))?
                .as_nanos();
            let id = format!("change-{}-{timestamp}-{sequence}", std::process::id());
            if !self.manifest_path(&id).exists() {
                return Ok(id);
            }
        }
        Err(repair_needed("无法分配唯一 workspace change_id"))
    }

    pub(crate) fn manifest_path(&self, change_id: &str) -> PathBuf {
        self.changes.join(format!("{change_id}.json"))
    }

    pub(crate) fn write_new(&self, filename: &str, contents: &[u8]) -> Result<(), ToolError> {
        write_new_file(&self.changes.join(filename), contents)
    }

    pub(crate) fn write_manifest(&self, change_id: &str, value: &Value) -> Result<(), ToolError> {
        write_atomic_json(&self.manifest_path(change_id), value)
    }

    pub(crate) fn read_manifest(&self, change_id: &str) -> Result<Value, ToolError> {
        read_manifest(&self.manifest_path(change_id))
    }

    pub(crate) fn read_manifest_path(&self, path: &Path) -> Result<Value, ToolError> {
        read_manifest(path)
    }

    pub(crate) fn read_preimage(&self, filename: &str) -> Result<Vec<u8>, ToolError> {
        fs::read(self.changes.join(filename))
            .map_err(|_| repair_needed("workspace journal preimage 缺失"))
    }
}

pub(crate) fn valid_change_id(change_id: &str) -> bool {
    change_id.starts_with("change-")
        && change_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub(crate) fn repair_needed(message: impl Into<String>) -> ToolError {
    tool_err("journal_needs_repair", message)
}

fn ensure_plain_dir(path: &Path) -> Result<(), ToolError> {
    if plain_dir_if_exists(path)? {
        return Ok(());
    }
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(journal_io(error)),
    }
    plain_dir_if_exists(path)?
        .then_some(())
        .ok_or_else(|| repair_needed("无法创建 workspace journal"))
}

fn plain_dir_if_exists(path: &Path) -> Result<bool, ToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(repair_needed("workspace journal 包含 symlink"))
        }
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(repair_needed("workspace journal 路径不是目录")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(journal_io(error)),
    }
}

fn write_new_file(path: &Path, contents: &[u8]) -> Result<(), ToolError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(journal_io)?;
    file.write_all(contents).map_err(journal_io)?;
    file.sync_all().map_err(journal_io)
}

fn write_atomic_json(path: &Path, value: &Value) -> Result<(), ToolError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| repair_needed(format!("无法编码 manifest：{error}")))?;
    let temp = path.with_extension(format!(
        "json.tmp-{}",
        JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    write_new_file(&temp, &bytes)?;
    fs::rename(&temp, path).map_err(journal_io)?;
    sync_directory(path.parent().expect("manifest has a parent"));
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Value, ToolError> {
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            tool_err("change_not_found", "找不到变更记录")
        } else {
            repair_needed(format!("无法读取 workspace journal：{error}"))
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|_| repair_needed("workspace journal manifest 损坏"))
}

fn sync_directory(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

fn journal_io(error: std::io::Error) -> ToolError {
    repair_needed(format!("workspace journal I/O 失败：{error}"))
}

fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}
