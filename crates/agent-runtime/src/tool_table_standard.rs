//! **web-agent 兼容的标准工具集这件事**：`standard_local` / `standard` 两档怎么装。
//!
//! 从 `tool_table.rs` 分出来的一件事（148 给表加第六个子模块时顶破 300 行，红线 9
//! 要求拆分就在本次改动里做完）。挑这一族出去而不是别的：它是表里**唯一不遵守
//! `<位置前缀>:<命名空间>/<工具>` 约定的**一族（`docs/TOOLS.md` §命名空间「两族
//! 并存」）——名字要跟既有 web-agent 的工具名逐字一致，改一个字就是一次全量前缀
//! 失效（红线 11），它自带一整套「为什么长成这样」的理由，跟内置五档那几个开闸的
//! 理由不是同一件事。
//!
//! 内置五档、`push_spec` 判重、`snapshot` 的三级判定留在 [`super`]。

use agent_core::ToolSpec;

use super::ToolTable;

impl ToolTable {
    /// web-agent 兼容的本地标准工具集：四个只读文件工具、受版本保护的工作区
    /// 事务、测试/lint 命令发现与六个静态命令工具。`read_file` 直接返回事务所需
    /// revision，因此模型不需要学习额外的内部前置工具。
    ///
    /// 此构造器不夹带历史 `srv:*` 别名，避免模型面对两套同义工具。浏览器交互工具
    /// 必须由 [`ToolTable::standard`] 的远程 router 注册，不能伪装为本地 executor。
    pub fn standard_local() -> Self {
        Self::from_specs(standard_local_specs())
    }

    /// 完整的 web-agent 标准工具集：本地工具外加三个由 Web 宿主执行并回传的交互
    /// 工具。它不注册计划、子 agent 或 MCP 工具。
    pub fn standard() -> Self {
        let mut specs = standard_local_specs();
        specs.extend(agent_tools::interaction_specs());
        Self::from_specs(specs)
    }
}

fn standard_local_specs() -> Vec<ToolSpec> {
    let mut specs = agent_tools::standard_readonly_file_specs();
    specs.extend(agent_tools::standard_workspace_file_specs());
    specs.push(agent_tools::find_test_lint_commands_spec());
    specs.extend(agent_tools::command_specs());
    specs
}

#[cfg(test)]
#[path = "standard_local_tests.rs"]
mod standard_local_tests;
