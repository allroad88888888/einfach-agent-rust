//! 工作区常规文件的确定性遍历。
//!
//! 这里唯一负责把用户给出的相对起点展开成一个有序的常规文件列表：绝不跟随
//! symlink，所有路径都在 executor 已 canonicalize 的 root 内。搜索语义（文件名
//! 或文本）留给调用方，避免把两个工具的规则混进同一个模块。

use crate::ToolError;
use crate::exec::{Resolved, resolve_in_root, tool_err};
use std::path::{Component, Path, PathBuf};

pub(crate) const MAX_VISITED_ENTRIES: usize = 20_000;

/// 确定性文件遍历的结果。`truncated` 表示到达固定遍历预算，结果不是完整集。
#[derive(Debug)]
pub(crate) struct FileWalk {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) truncated: bool,
}

/// 收集 `path`（省略时为 `.`）下的所有常规文件，按 workspace-relative 路径排序。
pub(crate) fn regular_files(root: &Path, path: Option<&str>) -> Result<FileWalk, ToolError> {
    let start = resolve_start(root, path.unwrap_or("."))?;
    let metadata = std::fs::symlink_metadata(&start)
        .map_err(|e| tool_err("not_found", format!("搜索起点不存在：{e}")))?;

    if metadata.is_file() {
        return Ok(FileWalk {
            files: vec![start],
            truncated: false,
        });
    }
    if !metadata.is_dir() {
        return Err(tool_err("bad_input", "搜索起点必须是文件或目录"));
    }

    let mut files = Vec::new();
    let mut pending = vec![start];
    let mut visited = 0usize;
    let mut truncated = false;

    while let Some(directory) = pending.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&directory)
            .map_err(|e| tool_err("bad_input", format!("读取目录失败：{e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| tool_err("bad_input", format!("读取目录失败：{e}")))?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            if visited == MAX_VISITED_ENTRIES {
                truncated = true;
                break;
            }
            visited += 1;

            let file_type = entry
                .file_type()
                .map_err(|e| tool_err("bad_input", format!("读取文件类型失败：{e}")))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_file() {
                files.push(entry.path());
            } else if file_type.is_dir() {
                pending.push(entry.path());
            }
        }

        if truncated {
            break;
        }
    }

    files.sort_by_key(|file| display_path(root, file));
    Ok(FileWalk { files, truncated })
}

/// 把已验证的文件路径转换为稳定的 `/` 分隔 workspace-relative 显示路径。
pub(crate) fn display_path(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .expect("walker 只能产生 root 下的路径")
        .to_string_lossy()
        .replace('\\', "/")
}

fn resolve_start(root: &Path, path: &str) -> Result<PathBuf, ToolError> {
    if path.is_empty() || path.contains('\0') {
        return Err(tool_err(
            "bad_input",
            "path 必须是非空且不含 NUL 的相对路径",
        ));
    }

    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(tool_err(
            "outside_root",
            "path 必须是相对 root 的路径，不能包含 ..",
        ));
    }
    reject_symlink_components(root, relative)?;

    match resolve_in_root(root, path)? {
        Resolved::Existing(path) => Ok(path),
        Resolved::Missing => Err(tool_err("not_found", format!("搜索起点不存在：{path}"))),
    }
}

fn reject_symlink_components(root: &Path, relative: &Path) -> Result<(), ToolError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(tool_err(
                    "bad_input",
                    "搜索起点不能是 symlink 或位于 symlink 下",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(tool_err("bad_input", format!("路径解析失败：{error}"))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::new_executor;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-tools-walk-{name}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_regular_files_in_relative_path_order() {
        let root = new_executor(temp_root("sort")).unwrap();
        std::fs::write(root.join("z.txt"), "z").unwrap();
        std::fs::create_dir_all(root.join("a/nested")).unwrap();
        std::fs::write(root.join("a/nested/x.txt"), "x").unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();

        let walk = regular_files(&root, None).unwrap();
        let names: Vec<_> = walk
            .files
            .iter()
            .map(|file| display_path(&root, file))
            .collect();
        assert_eq!(names, ["a.txt", "a/nested/x.txt", "z.txt"]);
        assert!(!walk.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinked_entries_and_rejects_a_symlink_start() {
        let root = new_executor(temp_root("symlink")).unwrap();
        std::fs::write(root.join("kept.txt"), "kept").unwrap();
        std::os::unix::fs::symlink(root.join("kept.txt"), root.join("linked.txt")).unwrap();

        let walk = regular_files(&root, None).unwrap();
        assert_eq!(walk.files.len(), 1);
        assert_eq!(display_path(&root, &walk.files[0]), "kept.txt");
        let err = regular_files(&root, Some("linked.txt")).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }
}
