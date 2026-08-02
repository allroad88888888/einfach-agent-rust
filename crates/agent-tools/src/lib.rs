//! 内置工具最小集（issue 013）：`srv:fs/read`、`srv:fs/list` 与本地 executor；
//! 外加独立声明（issue 020）的 `srv:shell/exec`——第一个
//! `Reversibility::Irreversible` 的工具，`builtin_specs()` **不含**它，见
//! `shell_spec()` 的文档。
//!
//! **本文件是公开 API 的唯一出口，只放签名与转发**——实现在子模块，
//! 独立测试 agent 只读这个文件（WORKFLOW §三：测试 agent 看不到实现体）。
//!
//! executor 返回**原始**输出，不截断——截断在 core 边界做
//! （`agent_core::truncate_tool_output`，决策 19），executor 不知道 prompt 预算。

use agent_core::ToolSpec;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

mod exec;
mod fs_list;
mod fs_read;
mod shell;
mod specs;

#[cfg(test)]
mod barrier_demo;

/// 工具执行失败。`code` 由工具自己定义（如 `not_found` / `outside_root` /
/// `bad_input`），进 `tool_result` 让模型决定要不要紧。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ToolError {
    pub code: Arc<str>,
    pub message: Arc<str>,
}

/// 内置工具表，**顺序固定**（fs/read 在前，fs/list 在后）。
/// 红线 11：这张表进 prompt 最前面，序列化必须逐字节确定——`Vec` + schema 走
/// `serde_json::Value`（Map 是 BTreeMap，key 有序）。
pub fn builtin_specs() -> Vec<ToolSpec> {
    specs::builtin_specs()
}

/// `srv:shell/exec` 的声明（issue 020）。**故意不在 `builtin_specs()` 里**：
/// 它是 `Reversibility::Irreversible`（这个等级不是 `ToolSpec` 的字段，是调用方
/// 工具表要标注的元数据，见 `docs/TOOLS.md`），没有 undo 屏障挡着就默认开着
/// 是数据事故——020 的范围只做工具本体，集成阶段连同 undo 屏障一起显式把它
/// 加进某个工具表。
pub fn shell_spec() -> ToolSpec {
    specs::shell_spec()
}

/// 本地文件系统 + shell executor。**路径监狱只覆盖文件类工具**：`fs/read`、
/// `fs/list` 的访问范围锁在 `root` 之内，越界（`..`、绝对路径逃逸、symlink
/// 穿透）一律拒绝；`shell/exec`（issue 020）没有这层监狱可言，只是把命令的
/// **工作目录起点**锁在 `root`，命令内容本身不受约束——见 `shell.rs` 顶部
/// 注释，这正是它被判 `Irreversible` 的理由。
///
/// 013 建成时叫 `FsExecutor`，020 落地时特意没跟着改名（改名要牵动
/// `agent-runtime`/`agent-cli` 里所有引用它的地方，那两个 crate 当时不在 020
/// 范围内）。027 换接 runner/CLI 到 `Session` 时顺手结这笔账——它现在分发的
/// 不只是 fs 工具，`ToolExecutor` 才是准确的名字，全仓引用一起改。
pub struct ToolExecutor {
    root: PathBuf,
}

impl ToolExecutor {
    /// `root` 会被 canonicalize；不存在或不是目录直接报错。
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ToolError> {
        exec::new_executor(root.into()).map(|root| ToolExecutor { root })
    }

    /// 按带命名空间的全名执行（`srv:fs/read` / `srv:fs/list` / `srv:shell/exec`）。
    /// 未知工具名返回 `code = "unknown_tool"` 的错误，不 panic。
    pub fn execute(&self, tool: &str, input: &Value) -> Result<String, ToolError> {
        exec::execute(&self.root, tool, input)
    }
}
