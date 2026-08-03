//! 可变更工作区目标的路径预检。
//!
//! 这里不打开文件，也不保证随后操作时文件系统没有被外部进程替换。未来的写入
//! 工具必须在持锁后再次验证并用 no-follow 原语打开目标；本模块只保证调用方不会
//! 因绝对路径、`..` 或当前已存在的 symlink 而把目标预先解析到工作区外。

use crate::ToolError;
use std::io;
use std::path::{Component, Path, PathBuf};

/// 已规范化的、相对于 canonical workspace root 的变更目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceTarget {
    relative: PathBuf,
    absolute: PathBuf,
}

impl WorkspaceTarget {
    /// 供锁键、journal 和工具结果使用的规范化相对路径。
    pub(crate) fn relative(&self) -> &Path {
        &self.relative
    }

    /// 供后续受保护文件操作使用的绝对目标路径。
    pub(crate) fn absolute(&self) -> &Path {
        &self.absolute
    }
}

/// 解析一个非空 workspace-relative 文件目标。
///
/// `root` 可以带 symlink；返回路径一律基于它的 canonical 目录。所有已经存在的
/// 路径段不得为 symlink，避免未来写入意外跟随链接。不存在的末尾路径段允许保留，
/// 以支持后续的“新建文件”操作。
pub(crate) fn parse_workspace_target(root: &Path, raw: &str) -> Result<WorkspaceTarget, ToolError> {
    let canonical_root = canonical_workspace_root(root)?;
    let relative = normalize_relative(raw)?;
    let absolute = canonical_root.join(&relative);
    reject_existing_symlink_components(&canonical_root, &relative)?;
    Ok(WorkspaceTarget { relative, absolute })
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf, ToolError> {
    let canonical = root
        .canonicalize()
        .map_err(|error| tool_err("bad_root", format!("root 无法解析：{error}")))?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(tool_err(
            "bad_root",
            format!("root 不是目录：{}", canonical.display()),
        ))
    }
}

fn normalize_relative(raw: &str) -> Result<PathBuf, ToolError> {
    if raw.is_empty() {
        return Err(tool_err("bad_input", "path 不能为空"));
    }

    let mut normalized = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(tool_err(
                    "outside_root",
                    "path 必须是 workspace-relative，且不能包含 `..` 或绝对路径",
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(tool_err("bad_input", "path 必须指向 root 内的文件"));
    }
    Ok(normalized)
}

fn reject_existing_symlink_components(root: &Path, relative: &Path) -> Result<(), ToolError> {
    let mut current = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(segment) = component else {
            unreachable!("normalize_relative only returns normal components");
        };
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(tool_err(
                    "unsafe_path",
                    format!("path 包含不允许写入的 symlink：{}", current.display()),
                ));
            }
            Ok(metadata) if index + 1 < components.len() && !metadata.is_dir() => {
                return Err(tool_err(
                    "bad_input",
                    format!("path 的父级不是目录：{}", current.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(tool_err("bad_input", format!("路径解析失败：{error}")));
            }
        }
    }
    Ok(())
}

fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_workspace_target;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-tools-workspace-target-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn normalizes_a_relative_target_below_canonical_root() {
        let root = temp_root("normalizes");
        let target = parse_workspace_target(&root, "src/./lib.rs").unwrap();

        assert_eq!(target.relative(), PathBuf::from("src/lib.rs"));
        assert_eq!(
            target.absolute(),
            root.canonicalize().unwrap().join("src/lib.rs")
        );
    }

    #[test]
    fn permits_a_missing_target_for_a_future_create_operation() {
        let root = temp_root("missing");
        let target = parse_workspace_target(&root, "new/deep/file.txt").unwrap();

        assert_eq!(target.relative(), PathBuf::from("new/deep/file.txt"));
    }

    #[test]
    fn rejects_empty_root_and_parent_escape_targets() {
        let root = temp_root("escape");
        for raw in ["", ".", "../outside.txt", "/etc/passwd"] {
            let error = parse_workspace_target(&root, raw).unwrap_err();
            assert!(matches!(&*error.code, "bad_input" | "outside_root"));
        }
    }

    #[test]
    fn rejects_a_file_used_as_a_parent_directory() {
        let root = temp_root("file-parent");
        std::fs::write(root.join("file.txt"), b"not a directory").unwrap();

        let error = parse_workspace_target(&root, "file.txt/child.txt").unwrap_err();
        assert_eq!(&*error.code, "bad_input");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_existing_symlink_components_even_when_they_stay_inside_root() {
        let root = temp_root("symlink");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();

        let error = parse_workspace_target(&root, "link/file.txt").unwrap_err();
        assert_eq!(&*error.code, "unsafe_path");
    }
}
