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

mod apply_patch_spec;
mod command_adapter;
mod command_discovery;
mod command_discovery_candidates;
mod command_discovery_specs;
mod command_plan;
mod command_specs;
mod exec;
mod fs_alias_specs;
mod fs_list;
mod fs_read;
mod fs_response;
mod fs_rg_search;
mod fs_search_files;
mod fs_walk;
mod git_diff_plan;
mod interaction_specs;
mod search_specs;
mod shell;
mod specs;
mod workspace;
mod workspace_specs;
mod workspace_standard_specs;

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

/// `srv:fs/search_files` 的独立声明。它目前不进入既有 `builtin_specs()`：
/// `agent-runtime` 尚未把它标为 `Pure`，不能在只改本 crate 的前提下让一个只读
/// 工具被误记成不可逆。主工具表迁移到 descriptor 后再显式启用它。
pub fn search_files_spec() -> ToolSpec {
    search_specs::search_files_spec()
}

/// `srv:fs/rg_search` 的独立声明；启用条件与 [`search_files_spec`] 相同。
pub fn rg_search_spec() -> ToolSpec {
    search_specs::rg_search_spec()
}

/// web-agent 兼容的标准只读文件工具声明。`read_file` 直接返回可用于写入的
/// revision；其余工具转发到已经验证的 `srv:fs/*` 实现。
pub fn standard_readonly_file_specs() -> Vec<ToolSpec> {
    fs_alias_specs::standard_readonly_file_specs()
}

/// 静态 shell、固定任务、验证命令与只读 Git diff 的声明。
///
/// 它们都必须经由运行时的不可逆权限策略显式启用；本函数只提供稳定的、模型可读的
/// descriptor，并不自动加入既有最小工具表。
pub fn command_specs() -> Vec<ToolSpec> {
    command_specs::command_specs()
}

/// 标准 `find_test_lint_commands` 的声明。它仅从受限 manifest 中发现候选 argv，
/// 绝不执行返回的命令；需要执行时应走具有独立权限语义的命令工具。
pub fn find_test_lint_commands_spec() -> ToolSpec {
    command_discovery_specs::find_test_lint_commands_spec()
}

/// 浏览器宿主执行的交互工具声明。它们不经过本地 [`ToolExecutor`]，运行时会把
/// 同一调用路由给 Web 并等待带原 `tool_call_id` 的受控回传。
pub fn interaction_specs() -> Vec<ToolSpec> {
    interaction_specs::interaction_specs()
}

/// `srv:fs/inspect` 的声明。先读取它返回的 revision，才可以调用可撤回写入工具。
pub fn inspect_spec() -> ToolSpec {
    workspace_specs::inspect_spec()
}

/// `srv:fs/write_text` 的声明。写入必须带 inspect 得到的 revision，拒绝盲写。
pub fn write_text_spec() -> ToolSpec {
    workspace_specs::write_text_spec()
}

/// `srv:workspace/revert_change` 的声明。用一次成功写入返回的 change_id 撤回。
pub fn revert_change_spec() -> ToolSpec {
    workspace_specs::revert_change_spec()
}

/// 标准名称的可撤回文件工具声明。它们使用 read_file 返回的 revision 防止两个
/// agent 对同一路径盲写；成功结果的 change_id 统一由 revert_workspace_change
/// 使用。
pub fn standard_workspace_file_specs() -> Vec<ToolSpec> {
    workspace_standard_specs::standard_workspace_file_specs()
}

/// 本地文件系统 + shell executor。**路径监狱覆盖文件与工作区工具**：读取、
/// 列表、搜索、inspect、标准 read_file 与写入都锁在 `root` 之内，越界（`..`、绝对路径逃逸、
/// symlink 穿透）一律拒绝；`shell/exec`（issue 020）没有这层监狱可言，只是
/// 把命令的**工作目录起点**锁在 `root`，命令内容本身不受约束——见 `shell.rs`
/// 顶部注释，这正是它被判 `Irreversible` 的理由。
///
/// 013 建成时叫 `FsExecutor`，020 落地时特意没跟着改名（改名要牵动
/// `agent-runtime`/`agent-cli` 里所有引用它的地方，那两个 crate 当时不在 020
/// 范围内）。027 换接 runner/CLI 到 `Session` 时顺手结这笔账——它现在分发的
/// 不只是 fs 工具，`ToolExecutor` 才是准确的名字，全仓引用一起改。
pub struct ToolExecutor {
    root: PathBuf,
    workspace: workspace::transaction::WorkspaceTransactionCoordinator,
}

impl ToolExecutor {
    /// `root` 会被 canonicalize；不存在或不是目录直接报错。
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ToolError> {
        let root = exec::new_executor(root.into())?;
        let workspace = workspace::transaction::WorkspaceTransactionCoordinator::new(root.clone())?;
        Ok(Self { root, workspace })
    }

    /// 按带命名空间的全名执行（`srv:fs/read`、`srv:fs/list`、搜索、shell，或
    /// `srv:fs/inspect` / `srv:fs/write_text` / `srv:workspace/revert_change`）。
    /// 未知工具名返回 `code = "unknown_tool"` 的错误，不 panic。
    pub fn execute(&self, tool: &str, input: &Value) -> Result<String, ToolError> {
        match tool {
            tool if command_adapter::is_static_command_tool(tool) => {
                command_adapter::execute(&self.root, tool, input)
            }
            "read_file" => workspace::tool_adapter::read_file(&self.workspace, input),
            "list_files" => exec::execute(&self.root, "srv:fs/list", input),
            "search_files" => exec::execute(&self.root, "srv:fs/search_files", input),
            "rg_search" => exec::execute(&self.root, "srv:fs/rg_search", input),
            "find_test_lint_commands" => {
                exec::execute(&self.root, "srv:workspace/find_test_lint_commands", input)
            }
            "srv:fs/inspect" => workspace::tool_adapter::inspect(&self.workspace, input),
            "srv:fs/write_text" => workspace::tool_adapter::write_text(&self.workspace, input),
            "write_file" => workspace::tool_adapter::write_text(&self.workspace, input),
            "delete_path" | "srv:workspace/delete_file" => {
                workspace::tool_adapter::delete_file(&self.workspace, input)
            }
            "copy_path" | "srv:workspace/copy_file" => {
                workspace::tool_adapter::copy_file(&self.workspace, input)
            }
            "move_path" | "srv:workspace/move_file" => {
                workspace::tool_adapter::move_file(&self.workspace, input)
            }
            "apply_patch" | "srv:workspace/apply_patch" => {
                workspace::tool_adapter::apply_patch(&self.workspace, input)
            }
            "srv:workspace/revert_change" => {
                workspace::tool_adapter::revert_change(&self.workspace, input)
            }
            "revert_workspace_change" => {
                workspace::tool_adapter::revert_change(&self.workspace, input)
            }
            _ => exec::execute(&self.root, tool, input),
        }
    }
}
