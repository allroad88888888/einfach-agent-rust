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
pub(crate) fn final_text(session: &Session, child: &AgentId) -> String {
    visible_text(session, child).unwrap_or_else(|| "（子 agent 没有产出任何文本）".to_string())
}

fn visible_text(session: &Session, child: &AgentId) -> Option<String> {
    let messages = session.messages_of(child);
    let last = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && has_text(m));
    last.map(|message| {
        message
            .blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(t) => Some(&**t),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn has_text(message: &Message) -> bool {
    message
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text(_)))
}

/// 097 验收第 3 条：单元级钉住 [`final_text`] 的块过滤——它必须只捞 `Text`，
/// 不管一条消息里还塞了多少 `ToolUse`/`ToolResult`，也不管子跑了多少轮。
///
/// **不起网络**：直接驱动 `Session::step`，比 `tests/it` 里那两份真起假 SSE
/// 服务器的集成测试（`blocking_spawn_omits_child_turns.rs` /
/// `collect_omits_child_turns.rs`）快得多——那两份测的是「这条性质在真实请求
/// 体上成立」，这份测的是「性质住的那个函数本身没写错」。
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{ChildConfig, Event, PrefixImage, StopReason, TokenUsage, ToolCallId};
    use serde_json::json;

    use super::*;

    /// 造一个子 agent，逼它连续做 `rounds` 轮「Text + ToolUse + ToolResult」——
    /// **同一条消息里三种块都塞**，不是真实流水线会长出的形状（真实的 ToolResult
    /// 落在收敛时另起的一条消息里，见 `crate::subtree` 与
    /// `agent_core::command::transitions::tool_outcome`）。刻意塞成这样是为了把
    /// `final_text` 的块过滤逼到极限：给它一条**同时含三种块**的消息，看它是不是
    /// 真的只捞 `Text`，而不是恰好因为「ToolResult 从不跟 Text 同框」才蒙对。
    ///
    /// 每轮之间补一次 `Event::ToolResult` 把子带回 `Thinking`——`ToolsPending`
    /// 状态下再来一条 `ProviderDone` 会被判 `ProtocolViolation`，不会真的落地成
    /// 消息（这份测试要的是「20 条 assistant 消息都在」，不能让转移表拒收）。
    fn child_after_n_rounds(rounds: u32) -> (Session, AgentId) {
        let mut session = Session::new(AgentId::root());
        let root = AgentId::root();
        session.step(Event::UserInput {
            agent: root.clone(),
            text: Arc::from("派一个子去跑几轮"),
        });
        let child = session
            .spawn_child(&root, ChildConfig::default(), None)
            .expect("root 是活的，深度/子数都在默认上限内");
        session.step(Event::UserInput {
            agent: child.clone(),
            text: Arc::from("开工"),
        });

        for i in 0..rounds {
            let marker = format!("CHILD_STEP_{i:02}");
            let call_id = ToolCallId::new(format!("call_{i}"));
            session.step(Event::ProviderDone {
                agent: child.clone(),
                epoch: session.epoch(),
                blocks: vec![
                    ContentBlock::Text(Arc::from(format!("{marker} 中间说明"))),
                    ContentBlock::ToolUse {
                        id: call_id.clone(),
                        name: Arc::from("srv:fs/read"),
                        input: Arc::new(json!({"path": "step.txt"})),
                    },
                    ContentBlock::ToolResult {
                        id: call_id.clone(),
                        content: Arc::from(format!("{marker} 工具原始产物")),
                        is_error: false,
                    },
                ],
                stop: StopReason::ToolUse,
                usage: TokenUsage {
                    prompt: 1,
                    completion: 1,
                    cached: None,
                },
                prefix: PrefixImage {
                    segments: Vec::new(),
                    prompt_tokens: None,
                },
                adjustments: Vec::new(),
            });
            session.step(Event::ToolResult {
                agent: child.clone(),
                epoch: session.epoch(),
                call_id,
                content: Arc::from("resolved"),
            });
        }

        (session, child)
    }

    #[test]
    fn final_text_is_only_the_last_assistant_texts_not_any_tool_use_or_tool_result() {
        let (session, child) = child_after_n_rounds(20);

        let text = final_text(&session, &child);

        assert_eq!(
            text, "CHILD_STEP_19 中间说明",
            "该等于最后一条 assistant 消息里 Text 块的拼接，一个字不多"
        );
        for i in 0..19 {
            let marker = format!("CHILD_STEP_{i:02}");
            assert!(
                !text.contains(&marker),
                "前面 19 轮的中间说明不该在场：{text}"
            );
        }
        assert!(
            !text.contains("工具原始产物"),
            "ToolResult 块的内容不该在场：{text}"
        );
        assert!(!text.contains("srv:fs/read"), "ToolUse 块不该在场：{text}");
    }

    #[test]
    fn final_text_length_does_not_grow_with_the_number_of_rounds() {
        let (session_5, child_5) = child_after_n_rounds(5);
        let (session_20, child_20) = child_after_n_rounds(20);

        let text_5 = final_text(&session_5, &child_5);
        let text_20 = final_text(&session_20, &child_20);

        assert_eq!(
            text_5.len(),
            text_20.len(),
            "终答只取决于最后一轮说了什么，不取决于跑了几轮：\
             text_5={text_5:?} text_20={text_20:?}"
        );
    }
}
