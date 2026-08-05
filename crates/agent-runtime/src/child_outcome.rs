//! 一个子 agent 的终态 → 父 agent 读得懂的一段文本（以及它算不算失败）。
//!
//! 从 `crate::subtree` 拆出来（053，红线 9）：那个文件管的是**记账**（谁在等谁、
//! 谁还在跑、谁跑完没人领），这里管的是**翻译**（子那边发生的事怎么变成父历史里
//! 的一条 tool_result 正文）。两件事之所以不该合住，是因为它们各自会往两个方向长
//! ——记账跟着编排走（前台/后台/collect 绑定），翻译跟着「父想读到什么」走
//! （截断说明、失败措辞、只取 `Text` 块）。
//!
//! # 这是**运行时侧读**，不是 core 跨读（红线 10）
//!
//! [`final_text`] 走的是 `Session::messages_of(child)`——宿主自己读会话状态，然后
//! 用一条 `Event::ToolResult` 从正门写回父。它不经 core 的跨 agent 读 API：
//! `Messages` 槽是 **Upward-only**，父在 core 那一层根本读不到子的正文
//! （ORCHESTRATION §五）。阻塞 spawn 从 029 起就是这条路，`collect`（053）复用它。

use agent_core::{AgentId, ContentBlock, Failure, Message, Role, Session, TurnStatus};

/// 子 agent 的终态 → 回给父的那段文本 + 它算不算失败。
///
/// **`is_error` = 子 Failed**（029 原文）。`Done { truncated: true }` 不算失败：
/// 它撞的是轮数闸，手上已经有半份答案，那份答案比一句「失败了」有用得多——003
/// 的哲学跨 agent 版，让模型看到全貌自己判断。前面加一行固定的说明让它知道
/// 这份答案是被截断的（固定文本，不带任何时间/计数，红线 11）。
pub(crate) fn outcome(session: &Session, child: &AgentId, status: &TurnStatus) -> (String, bool) {
    match status {
        TurnStatus::Done { truncated: false } => (final_text(session, child), false),
        TurnStatus::Done { truncated: true } => (
            format!(
                "[子 agent 撞到轮数上限，下面是它停下时的最后回复]\n{}",
                final_text(session, child)
            ),
            false,
        ),
        TurnStatus::Failed(Failure::Cancelled) => {
            ("子 agent 被取消，没有产出结果。".to_string(), true)
        }
        TurnStatus::Failed(Failure::Provider(class)) => (
            format!("子 agent 失败（provider {class:?}），没有产出结果。"),
            true,
        ),
        // 泵只在终态才收割，非终态在这里是不可达的——但 `TurnStatus` 是公开枚举，
        // 用 `unreachable!` 换一句诚实的兜底文本：一条奇怪的 tool_result 比一次
        // panic 好，父 agent 至少还能继续。
        other => (
            format!("子 agent 停在非终态 {other:?}，没有产出结果。"),
            true,
        ),
    }
}

/// 子 agent 的最后一条 assistant 消息里的可见文本。
///
/// 只取 `Text` 块：`Thinking` 是它的思考过程（要不要进 prompt 是 adapter 的判断，
/// 不该由我们替父 agent 决定），`ToolUse` / `ToolResult` 是它的干活痕迹，父 agent
/// 要的是结论。一条消息里多个 `Text` 块按顺序换行拼接。
fn final_text(session: &Session, child: &AgentId) -> String {
    let messages = session.messages_of(child);
    let last = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && has_text(m));
    match last {
        Some(message) => message
            .blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(t) => Some(&**t),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => "（子 agent 没有产出任何文本）".to_string(),
    }
}

fn has_text(message: &Message) -> bool {
    message
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text(_)))
}
