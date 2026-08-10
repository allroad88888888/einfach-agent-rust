//! 投影：`(完整历史, SendPlan, 摘要正文) → 要发的历史`。
//!
//! [`SendPlan`] 是坐标，这里是把坐标用出去的那一步。**纯函数**（红线 1）——零 IO、
//! 零时钟、零随机、不读全局可变状态，同一份入参投一千次逐字节相同。这条不是洁癖：
//! 它不纯，就不能当 derived，「压缩只发生在发送侧」整个方案跟着塌。

use std::collections::BTreeSet;
use std::sync::Arc;

use imbl::Vector;

use crate::ids::{MessageId, ToolCallId};
use crate::value::message::{ContentBlock, Message, Role};

use super::SendPlan;

/// 被清除的工具结果在 prompt 里的占位文本。
///
/// **逐字节确定**（红线 11）：只有固定文本，无时间戳、无 id、无大小数字。
/// 跟 004 的截断标记同一套纪律——任何随输入浮动的字节都会把前缀缓存打散。
pub const CLEARED_TOOL_RESULT: &str = "（工具结果已清除以腾出上下文；需要请重新调用）";

/// 合成摘要消息的 id。
///
/// `MessageId(0)` 永远不会跟真消息撞：`Slot::NextMessageId` 默认 `1`、`mint` 只增
/// （`command/txn.rs`），历史里的 id 从 1 起。
///
/// 要一个**固定**值而不是现铸「比最大还大 1」，理由是**投影的输出只该取决于
/// `(历史内容, plan)`，不该取决于历史的 id 编号**：内容相同、id 编号不同的两份
/// 历史必须投出相等的结果，否则 `PartialEq` 判等和任何 golden 断言都会随
/// 编号漂。
///
/// **不是**「否则破坏逐字节确定性」——现铸方案对固定入参照样确定，
/// 那条测试抓不到它；`MessageId` 也不进 wire（`wire/messages.rs` 只在测试里
/// 用到它），所以跟前缀缓存无关。这个理由写错过一次，留着这段免得再错一次。
const SUMMARY_MESSAGE_ID: MessageId = MessageId(0);

/// 把 `plan` 作用到 `history` 上，得到这一轮真正要发的历史。
///
/// - `summary_text` 由调用方从别处取好再传进来——摘要正文是大值，不住 [`SendPlan`]
///   里（红线 5）。传 `None` 而 `plan.summary()` 是 `Some` 时，视为**摘要还没到**，
///   边界不生效：宁可多发，不可发一段引用不到正文的空洞。
/// - 清除工具结果 = 把 [`ContentBlock::ToolResult`] 的 `content` 换成
///   [`CLEARED_TOOL_RESULT`]，**`ToolUse` 与 `ToolResult` 块都留在原地**。这样配对
///   天然不破（有的 provider 见到落单的 `ToolUse` 直接 400），而 `ToolUse.input`
///   通常远小于结果正文，省下的还是大头。模型也因此知道自己调过这个工具、结果
///   没了、要用得重调——比假装没调过更诚实。
/// - 投影后为空的消息（块列表空）整条丢弃——空消息发出去也是 400。
/// - 摘要作为一条 [`Role::User`] 消息出现在最前面：它是「先前发生过什么」的转述，
///   多数 provider 又要求首条是 user，放这个角色最不容易在接缝处组出非法请求。
pub fn project(
    history: &Vector<Message>,
    plan: &SendPlan,
    summary_text: Option<&Arc<str>>,
) -> Vec<Message> {
    // 摘要引用与正文必须同时在。缺正文 → 边界作废，整份历史照发。
    let summary = plan.summary().and(summary_text);
    let boundary = if plan.summary().is_some() && summary.is_none() {
        0
    } else {
        plan.boundary()
    };

    // 只用于查表，不落盘、不进 prompt——`BTreeSet` 而非 `HashSet` 是顺手守住
    // 红线 11 的习惯：这个文件里不该出现任何顺序不定的容器。
    let cleared: BTreeSet<&ToolCallId> = plan.cleared().iter().collect();

    let mut out = Vec::with_capacity(history.len().saturating_sub(boundary) + 1);
    if let Some(text) = summary {
        out.push(Message {
            id: SUMMARY_MESSAGE_ID,
            role: Role::User,
            blocks: vec![ContentBlock::Text(Arc::clone(text))],
        });
    }
    for message in history.iter().skip(boundary) {
        if message.blocks.is_empty() {
            continue;
        }
        out.push(clear_results(message, &cleared));
    }
    out
}

/// 一条消息里被点名的 `ToolResult` 换正文，别的块原样。
fn clear_results(message: &Message, cleared: &BTreeSet<&ToolCallId>) -> Message {
    let blocks = message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult {
                id,
                content: _,
                is_error,
            } if cleared.contains(id) => ContentBlock::ToolResult {
                id: id.clone(),
                content: Arc::from(CLEARED_TOOL_RESULT),
                // `is_error` 保持原样：结果没了不代表当时没出错，翻成 `false`
                // 等于替模型改了一次历史判断。
                is_error: *is_error,
            },
            other => other.clone(),
        })
        .collect();
    Message {
        id: message.id,
        role: message.role.clone(),
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::ids::SummaryId;

    use super::*;

    fn text_msg(id: u64, role: Role, s: &str) -> Message {
        Message {
            id: MessageId(id),
            role,
            blocks: vec![ContentBlock::Text(Arc::from(s))],
        }
    }

    /// 一次工具往返：请求块 + 结果块，同一条消息里。
    fn tool_msg(id: u64, call: &str) -> Message {
        Message {
            id: MessageId(id),
            role: Role::Assistant,
            blocks: vec![
                ContentBlock::ToolUse {
                    id: ToolCallId::new(call),
                    name: Arc::from("fs/read"),
                    input: Arc::new(json!({ "path": "/tmp/a" })),
                },
                ContentBlock::ToolResult {
                    id: ToolCallId::new(call),
                    content: Arc::from("一大段工具输出"),
                    is_error: false,
                },
            ],
        }
    }

    fn history() -> Vector<Message> {
        Vector::from(vec![
            text_msg(1, Role::User, "问题"),
            tool_msg(2, "call_a"),
            tool_msg(3, "call_b"),
            text_msg(4, Role::Assistant, "回答"),
        ])
    }

    /// 恒等元：空 plan 投出来**等于**完整历史，一条不多一条不少。
    #[test]
    fn pristine_plan_projects_the_full_history() {
        let h = history();
        let out = project(&h, &SendPlan::new(), None);
        assert_eq!(out, h.iter().cloned().collect::<Vec<_>>());
    }

    /// 红线 1 / 红线 11：同一份入参投一千次，逐字节相同。
    #[test]
    fn projection_is_byte_identical_across_a_thousand_runs() {
        let h = history();
        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_a")]);
        plan.advance_boundary(1, Some(SummaryId::new("s1")))
            .unwrap();
        let text: Arc<str> = Arc::from("先前聊过的摘要");

        let first = serde_json::to_string(&project(&h, &plan, Some(&text))).unwrap();
        for _ in 0..1000 {
            assert_eq!(
                serde_json::to_string(&project(&h, &plan, Some(&text))).unwrap(),
                first
            );
        }
    }

    /// 第 2 档：`ToolResult` 换占位，`ToolUse` 原样——两边 id 集合恒等，
    /// 任何输入下都不出现落单的一半。
    #[test]
    fn clearing_swaps_the_result_and_keeps_the_tool_use() {
        let h = history();
        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_a")]);
        let out = project(&h, &plan, None);

        let mut uses = Vec::new();
        let mut results = Vec::new();
        for msg in &out {
            for block in &msg.blocks {
                match block {
                    ContentBlock::ToolUse { id, .. } => uses.push(id.clone()),
                    ContentBlock::ToolResult { id, content, .. } => {
                        results.push(id.clone());
                        let expected = if id.0.as_ref() == "call_a" {
                            CLEARED_TOOL_RESULT
                        } else {
                            "一大段工具输出"
                        };
                        assert_eq!(content.as_ref(), expected);
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(uses, results);
        assert_eq!(uses.len(), 2);
        // 消息条数不变：清的是块的正文，不是块、更不是消息。
        assert_eq!(out.len(), h.len());
    }

    /// 已清列表里有历史上不存在的 id：什么都不发生，不 panic。
    #[test]
    fn clearing_an_unknown_id_is_a_no_op() {
        let h = history();
        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_zzz")]);
        assert_eq!(
            project(&h, &plan, None),
            project(&h, &SendPlan::new(), None)
        );
    }

    /// 块列表空的消息整条不出现——空消息发出去是 400。
    #[test]
    fn empty_messages_are_dropped_entirely() {
        let mut h = history();
        h.push_back(Message {
            id: MessageId(5),
            role: Role::Assistant,
            blocks: Vec::new(),
        });
        let out = project(&h, &SendPlan::new(), None);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|m| m.id != MessageId(5)));
    }

    /// 有摘要引用、没摘要正文：**边界不生效**，投出完整历史，
    /// 而不是一段引用不到正文的空洞。
    #[test]
    fn a_missing_summary_text_disables_the_boundary() {
        let h = history();
        let mut plan = SendPlan::new();
        plan.advance_boundary(3, Some(SummaryId::new("s1")))
            .unwrap();
        assert_eq!(
            project(&h, &plan, None),
            h.iter().cloned().collect::<Vec<_>>()
        );
    }

    /// 边界之前的消息不出现；摘要正文在时，它是最前面那一条。
    #[test]
    fn the_summary_leads_and_the_prefix_is_gone() {
        let h = history();
        let mut plan = SendPlan::new();
        plan.advance_boundary(3, Some(SummaryId::new("s1")))
            .unwrap();
        let text: Arc<str> = Arc::from("先前聊过的摘要");
        let out = project(&h, &plan, Some(&text));

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[0].blocks, vec![ContentBlock::Text(Arc::clone(&text))]);
        assert_eq!(out[1].id, MessageId(4));
        assert!(out.iter().all(|m| m.id != MessageId(1)));
    }

    /// 无摘要引用的第 4 档（清窗口）：边界照样生效，只是前面没有摘要那条。
    /// 边界越界也不 panic，退化成「一条正文都不发」。
    #[test]
    fn a_boundary_without_a_summary_just_drops_the_prefix() {
        let h = history();
        let mut plan = SendPlan::new();
        plan.advance_boundary(4, None).unwrap();
        assert!(project(&h, &plan, None).is_empty());

        plan.advance_boundary(99, None).unwrap();
        assert!(project(&h, &plan, Some(&Arc::from("被忽略"))).is_empty());
    }
}
