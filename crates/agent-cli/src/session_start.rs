//! 135：CLI 侧「新建会话才跑开局工具」的接线薄壳。
//!
//! 真正的驱动是 `agent_runtime::run_session_start`（135 的执行体，见它的模块
//! 文档「全有或全无」）——这里只做两件事：只在新建会话那一支调它，把
//! `SessionStartError` 翻成一句给用户看的话交回 `main`（`main.rs` 保持只装配、
//! 不判断的既有原则，见那个文件的头部注释）。

use agent_core::Session;
use agent_runtime::{run_session_start, ToolTable};

/// `is_new_session` 为假（恢复路径）时是空操作——134 的状态已经带着上一次
/// 的值，重跑一遍等于用「此刻的外部世界」覆盖「那一刻的外部世界」给出的
/// 答案（`run_session_start` 模块文档「只有新建会话才跑这条路」）。
///
/// 任一开局工具失败 = 会话创建失败：返回 `Err`，调用方按启动失败处理
/// （`main` 用它已有的 `fail` 打印后非零码退出）——「全有或全无」由
/// `run_session_start` 自己保证，这一层只负责把 `SessionStartError` 转成
/// 一句话。
pub fn maybe_run(
    is_new_session: bool,
    session: &mut Session,
    tools: &ToolTable,
) -> Result<(), String> {
    if !is_new_session {
        return Ok(());
    }
    run_session_start(session, tools)
        .map_err(|e| format!("开局工具 {} 失败：{}", e.tool, e.message))
}
