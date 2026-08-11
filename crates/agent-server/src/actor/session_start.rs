//! 135：actor 侧「新建会话才跑开局工具」的接线薄壳，逐句对齐
//! `agent-cli` 的 `session_start` 模块（同一件事，两个宿主各自一份薄壳，
//! 理由同 `capabilities.rs` 顶部「分出来的判据」——`body.rs` 说得清的一句
//! 话是「actor 线程跑什么」，装不下这一段的理由）。
//!
//! 真正的驱动是 `agent_runtime::run_session_start`（见它的模块文档「全有或
//! 全无」）。这里只做一件事：只在**新建**那一支调它——`body.rs` 用
//! `restored`（`matches!(recovered, Ok(Some(_)))`）分辨这次是恢复还是新建，
//! 跟 073/064 的声明记录用的是同一个变量。

use agent_core::Session;
use agent_runtime::{run_session_start, ToolTable};

/// `restored` 为真（恢复路径）时是空操作——134 的状态已经带着上一次的值。
///
/// 任一开局工具失败 = 会话创建失败：返回 `Err`，调用方按既有的「actor 启动
/// 阶段的失败」路径处理（`body::run` 写 `ready_tx` 的 `Err` 支、线程直接
/// `return`，构造性错误一直是这么报的，见那个文件里 `ToolExecutor::new`
/// 失败的同款早退）。
pub(super) fn maybe_run(
    restored: bool,
    session: &mut Session,
    tools: &ToolTable,
) -> Result<(), String> {
    if restored {
        return Ok(());
    }
    run_session_start(session, tools)
        .map_err(|e| format!("开局工具 {} 失败：{}", e.tool, e.message))
}
