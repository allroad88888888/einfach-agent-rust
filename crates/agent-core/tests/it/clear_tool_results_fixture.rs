//! `clear_tool_results_*` 系列测试共用的 fixture：造一个含 `n` 轮
//! 「user → ToolUse → ToolResult → EndTurn」的 root 会话，每轮恰好一次工具调用。
//! 只负责「造一份确定的输入」，不含断言——跟 `support/mod.rs` 顶部同一条纪律。
//!
//! 独立于 `support/` 目录：104 的独立测试 agent 在并行改同一份 `support/`，
//! 这个文件只服务 101 的 `clear_tool_results_*` 系列，放在 `tests/it/` 顶层
//! 直接当模块用，避免两边同时改一个共享文件。

#![allow(dead_code)]

use agent_core::{Session, ToolCallId};

use crate::support::session::new_session;
use crate::support::{
    provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event,
};

/// `n` 轮工具调用，`call_id` 依次是 `call_0..call_{n-1}`（铸造顺序 = 调用顺序），
/// 各自的结果内容是 `result_{i}`——互不相同，方便测试拿内容反查「这一条」还在不在。
///
/// 返回驱动完的会话，以及按调用顺序排列的 `ToolCallId` 列表。
pub fn session_with_n_tool_calls(n: usize) -> (Session, Vec<ToolCallId>) {
    let mut session = new_session();
    let mut ids = Vec::with_capacity(n);

    for i in 0..n {
        let call_id = format!("call_{i}");
        let _ = session.step(user_input_event(&format!("调用工具 {i}")));
        let _ = session.step(provider_done_tool_use(
            session.epoch(),
            &[(call_id.as_str(), "srv:fs/read")],
        ));
        let _ = session.step(tool_result_event(
            session.epoch(),
            call_id.as_str(),
            &format!("result_{i}"),
        ));
        let _ = session.step(provider_done_end_turn(session.epoch(), "完成"));
        if i + 1 < n {
            session.begin_turn();
        }
        ids.push(ToolCallId::new(call_id));
    }

    (session, ids)
}
