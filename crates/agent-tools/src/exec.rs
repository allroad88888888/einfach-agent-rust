//! 路径监狱与分发。
//!
//! **路径监狱**：一切文件访问在 canonicalize 之后必须落在 canonical root 之内，
//! 否则拒绝（`code = "outside_root"`）。这同时挡住 `../` 逃逸和 symlink 穿透——
//! 两者的共同点是「lexical 路径看着在 root 里，canonicalize 之后跑出去了」。
//!
//! 读不存在的文件时，目标路径本身 canonicalize 会失败（没有东西可以 stat）。
//! 这时退而 canonicalize 离它最近的、存在的祖先目录来判监狱：祖先落在 root
//! 里，就是「合法路径，只是还没有这个文件」（`Resolved::Missing`）；祖先落在
//! root 外，就是「压根不该看这里」（`outside_root`）。这一步顺带挡住了绝对
//! 路径逃逸——`root.join("/etc/passwd")` 在 `Path::join` 语义下会整个替换掉
//! root，但替换之后的结果一样要过这个祖先检查，一样会被判定跑出了 root。

use crate::ToolError;
use crate::{fs_list, fs_read, shell};
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `root` 之下 canonicalize 之后的路径解析结果。
#[derive(Debug)]
pub(crate) enum Resolved {
    /// 已存在，携带它的 canonical 路径。
    Existing(PathBuf),
    /// 确认落在 root 之内，但目标本身不存在——报什么错由调用方决定
    /// （`fs/read` / `fs/list` 都报 `not_found`）。
    Missing,
}

/// 构造 executor 时初始化 root：canonicalize，校验它是一个目录。
pub(crate) fn new_executor(root: PathBuf) -> Result<PathBuf, ToolError> {
    let canonical = root
        .canonicalize()
        .map_err(|e| tool_err("bad_root", format!("root 无法解析：{e}")))?;
    if !canonical.is_dir() {
        return Err(tool_err(
            "bad_root",
            format!("root 不是目录：{}", canonical.display()),
        ));
    }
    Ok(canonical)
}

/// 按带命名空间的全名分发。`root` 已是 canonicalize 过的（`new_executor` 的产物）。
///
/// `srv:shell/exec`（issue 020）不做路径监狱：它的隔离只有「起点锁在 root」
/// （`current_dir`），命令内容本身不受约束——这正是它被判 `Irreversible`
/// 而不是复用 `resolve_in_root` 那套的原因，见 `shell.rs` 顶部注释。
pub(crate) fn execute(root: &Path, tool: &str, input: &Value) -> Result<String, ToolError> {
    match tool {
        "srv:fs/read" => fs_read::read(root, input),
        "srv:fs/list" => fs_list::list(root, input),
        "srv:shell/exec" => shell::execute(root, input),
        other => Err(tool_err("unknown_tool", format!("未知工具：{other}"))),
    }
}

/// 路径监狱核心：把 `rel` 相对 `root` 拼接并 canonicalize，校验结果落在
/// `root` 之内。`root` 必须已经是 canonical 的（`new_executor` 的产物）。
pub(crate) fn resolve_in_root(root: &Path, rel: &str) -> Result<Resolved, ToolError> {
    let joined = root.join(rel);
    match joined.canonicalize() {
        Ok(canon) if canon.starts_with(root) => Ok(Resolved::Existing(canon)),
        Ok(_) => Err(outside_root()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => resolve_missing_ancestor(root, &joined),
        Err(e) => Err(tool_err("bad_input", format!("路径解析失败：{e}"))),
    }
}

/// `joined` 本身 canonicalize 失败（不存在）时，往上找最近一个存在的祖先目录
/// 来判监狱。`root` 本身必存在（已 canonicalize 过），所以这个循环保证终止。
fn resolve_missing_ancestor(root: &Path, joined: &Path) -> Result<Resolved, ToolError> {
    let mut cur = joined.parent();
    loop {
        let Some(p) = cur else {
            return Err(outside_root());
        };
        match p.canonicalize() {
            Ok(canon) => {
                return if canon.starts_with(root) {
                    Ok(Resolved::Missing)
                } else {
                    Err(outside_root())
                };
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => cur = p.parent(),
            Err(e) => return Err(tool_err("bad_input", format!("路径解析失败：{e}"))),
        }
    }
}

fn outside_root() -> ToolError {
    tool_err("outside_root", "路径越界：不能访问 root 之外的内容")
}

/// 构造 `ToolError` 的小工具，`fs_read` / `fs_list` 共用。
pub(crate) fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: Arc::from(code),
        message: Arc::from(message.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 每个用例一个独立临时目录，避免测试间互相干扰。不做清理——临时目录
    /// 由 OS/CI 环境自行回收，测试不依赖这一点。
    fn temp_root(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-tools-exec-{name}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_executor_canonicalizes_root() {
        let dir = temp_root("new-ok");
        let got = new_executor(dir.clone()).unwrap();
        assert_eq!(got, dir.canonicalize().unwrap());
    }

    #[test]
    fn new_executor_rejects_missing_root() {
        let dir = temp_root("new-missing").join("does-not-exist");
        let err = new_executor(dir).unwrap_err();
        assert_eq!(&*err.code, "bad_root");
    }

    #[test]
    fn new_executor_rejects_file_as_root() {
        let dir = temp_root("new-file");
        let file = dir.join("not-a-dir.txt");
        std::fs::write(&file, b"x").unwrap();
        let err = new_executor(file).unwrap_err();
        assert_eq!(&*err.code, "bad_root");
    }

    #[test]
    fn execute_unknown_tool_is_rejected_not_panicked() {
        let root = new_executor(temp_root("dispatch-unknown")).unwrap();
        let err = execute(&root, "srv:fs/write", &json!({})).unwrap_err();
        assert_eq!(&*err.code, "unknown_tool");
    }

    #[test]
    fn resolve_existing_file_inside_root() {
        let root = new_executor(temp_root("resolve-existing")).unwrap();
        std::fs::write(root.join("a.txt"), b"hi").unwrap();
        match resolve_in_root(&root, "a.txt").unwrap() {
            Resolved::Existing(p) => assert_eq!(p, root.join("a.txt").canonicalize().unwrap()),
            Resolved::Missing => panic!("expected Existing"),
        }
    }

    #[test]
    fn resolve_missing_file_inside_root_is_missing_not_error() {
        let root = new_executor(temp_root("resolve-missing")).unwrap();
        assert!(matches!(
            resolve_in_root(&root, "nope.txt").unwrap(),
            Resolved::Missing
        ));
    }

    #[test]
    fn resolve_rejects_dotdot_escape() {
        let base = temp_root("resolve-dotdot");
        std::fs::create_dir_all(base.join("root")).unwrap();
        let root = new_executor(base.join("root")).unwrap();
        std::fs::write(base.join("outside.txt"), b"secret").unwrap();
        let err = resolve_in_root(&root, "../outside.txt").unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }

    #[test]
    fn resolve_rejects_dotdot_escape_to_missing_target() {
        // 越界的目标即使不存在，也必须先判监狱再判存在性——不能因为
        // canonicalize 失败就退化成 not_found。
        let base = temp_root("resolve-dotdot-missing");
        std::fs::create_dir_all(base.join("root")).unwrap();
        let root = new_executor(base.join("root")).unwrap();
        let err = resolve_in_root(&root, "../not-here-either.txt").unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }

    #[test]
    fn resolve_rejects_absolute_path_escape() {
        let root = new_executor(temp_root("resolve-abs")).unwrap();
        // 绝对路径在 Path::join 语义下会顶掉 root——监狱检查必须兜住它。
        let err = resolve_in_root(&root, "/etc/passwd").unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_symlink_escape() {
        let base = temp_root("resolve-symlink");
        std::fs::create_dir_all(base.join("root")).unwrap();
        let root = new_executor(base.join("root")).unwrap();
        let outside_target = base.join("outside-dir");
        std::fs::create_dir_all(&outside_target).unwrap();
        std::fs::write(outside_target.join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(&outside_target, root.join("escape")).unwrap();

        let err = resolve_in_root(&root, "escape/secret.txt").unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }
}
