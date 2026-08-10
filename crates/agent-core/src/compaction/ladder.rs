//! [`next_action`]：这一轮该走哪一档（108，096 的宏观部分）。
//!
//! **自动阶梯里只有第 2、3 档。** 另外三档在这里没有任何入口，而且是刻意的
//! （096 §七）：第 1 档（截断）常开、不看压力；第 4 档（清窗口）是用户动作、
//! 不受阈值管；第 5 档（子 agent）是结构性的、编排时就定了。这个函数的返回类型
//! [`LadderAction`] 只有三个变体，所以「往阶梯里再加一档」不是改个 `if`，
//! 是改一个公开枚举——那正是 096 说的「要写死，不然以后有人往里加」。
//!
//! **纯函数**（红线 1）：零 IO、零时钟、零随机，只看五个入参。同一份历史重放两次
//! 必然做出同一个决定（含触发了哪一档、清了哪些 id、边界推到哪）。
//!
//! **零 provider 分支**（红线 12）：阶梯顺序是产品判断，跟模型无关。这里连
//! 「哪家 provider」这个概念都进不来——入参里根本没有它。
//!
//! # ⚠️ 阶梯是「跨轮」的，不是同一轮里先清再摘
//!
//! 「第 2 档清完还不够」这句话**没法在同一轮里判**：清完之后新的 token 数要等下
//! 一轮实测才知道，而估算 token 需要 tokenizer——那是模型相关知识，写进 core 当场
//! 破红线 12（004 也早写过「字节数和 token 数关系不稳定」）。
//!
//! 所以阶梯是**时间上的**：这一轮清工具结果，下一轮再测；还超就说明清不动了，
//! 那时第 2 档自然返回空（都在 `plan.cleared()` 里了），第 3 档接手。触发线仍是
//! 85%，**一轮的代价换一个不用猜的判据**。
//!
//! 这个性质在这个文件里的形状就是：一次调用只返回**一个**动作，永远不会是
//! 「先清再摘」。让它一轮里连开两档的实现，会在同一轮里付两次全价重编码，而且
//! 第 3 档那次摘要吃的是一份刚被打上占位符的历史。
//!
//! # ⚠️ Y=30% 不是一个可计算的参数
//!
//! 096 定的「压到 30%」**没法作为停止条件实现**，同上：压之前算不出压之后是多少。
//! 两档的实际动作都是「把保护区之外的**全部**处理掉」（096 决策记录第三问的
//! 「一次全清」，以及用户对主动摘要那句「压到多少算多少」）。**30% 因此是预期
//! 落点不是输入参数**——由 110 真机量出来验证，不进代码。这也是
//! [`ClearParams`] 里只有 `trigger_percent` 一个水位的原因。

use imbl::Vector;

use crate::ids::ToolCallId;
use crate::value::message::Message;
use crate::value::send_plan::SendPlan;

use super::clear_policy::{ClearParams, tool_results_to_clear};
use super::pressure::over_trigger_line;
use super::protected_region::protected_region_start;

/// 这一轮该走哪一档。自动阶梯里只有第 2、3 档。
#[derive(Clone, PartialEq, Debug)]
pub enum LadderAction {
    /// 不压。
    Nothing,
    /// 第 2 档：清这批工具结果。
    ClearToolResults(Vec<ToolCallId>),
    /// 第 3 档：把 `[0, upto)` 摘要掉。
    Summarize { upto: usize },
}

/// 这一轮该走哪一档。
///
/// # 判定顺序（就三步，没有第四步）
///
/// 1. 压力**没超** `params.trigger_percent` → [`LadderAction::Nothing`]。
/// 2. 超了，且 [`tool_results_to_clear`] 返回非空 → [`LadderAction::ClearToolResults`]
///    （**第 2 档优先，永远**）。
/// 3. 超了，但第 2 档**已经无可清**（返回空）→
///    [`LadderAction::Summarize`]`{ upto: 保护区起点 }`；保护区起点为 `0`
///    （没东西可摘）时 → [`LadderAction::Nothing`]。
///
/// 第 2 档看百分比、第 3 档看**状态条件**（「第 2 档清光了还不够」）是 096 第二问
/// 定的：少一个魔法数字，而且语义正好等于要的阶梯——**便宜的招用尽才动贵的**。
///
/// 入参跟 [`tool_results_to_clear`] 逐个相同，不多不少：多出来的任何一个（时钟、
/// provider、已经压过几次）都会让「重放两次决定相同」不再是结构事实。
///
/// `last_prompt_tokens` = 上一轮**实测**的 prompt token 数，`None`（首轮 / 这家
/// provider 不报）不触发；`context_window` = `SessionConfig.context_window`，
/// `None`（未知/不设限）不触发，**不许 `unwrap`**。
pub fn next_action(
    history: &Vector<Message>,
    plan: &SendPlan,
    last_prompt_tokens: Option<u32>,
    context_window: Option<u32>,
    params: ClearParams,
) -> LadderAction {
    // 一。压力没超：两档都不开火。这一句同时是「`context_window: None` 时一档都
    // 不触发」的落点（`over_trigger_line` 对三种问不出来的输入一律答 false）。
    if !over_trigger_line(last_prompt_tokens, context_window, params.trigger_percent) {
        return LadderAction::Nothing;
    }

    // 二。第 2 档优先，**永远**。够得着的工具结果还有一个没清，就轮不到摘要——
    // 清工具结果不花模型调用，摘要要烧一次；顺序反了或者两档并行开火，整个
    // 「便宜的招用尽才动贵的」就没了（108 验收第一条专门为这一句写的）。
    let clearable = tool_results_to_clear(history, plan, last_prompt_tokens, context_window, params);
    if !clearable.is_empty() {
        return LadderAction::ClearToolResults(clearable);
    }

    // 三。第 2 档无可清 = 「清光了还不够」，第 3 档接手。摘的那一段跟第 2 档够得着
    // 的范围**是同一条线**（`protected_region_start`）：最近 N 轮不动，理由跟
    // 096 §四第 3 条一样——把近期也摘掉，模型会断片、原地重做刚做过的事。
    let upto = protected_region_start(history, params.protect_recent_turns);
    if upto == 0 {
        // 保护区起点为 0：整个历史都在保护区（`User` 消息不足 N 条），没东西可摘。
        // 摘一段空的会让边界前进而 prompt 里少一段真内容，比不压更糟。
        return LadderAction::Nothing;
    }
    LadderAction::Summarize { upto }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::ids::MessageId;
    use crate::value::message::{ContentBlock, Role};

    use super::*;

    fn user_msg(id: u64, text: &str) -> Message {
        Message {
            id: MessageId(id),
            role: Role::User,
            blocks: vec![ContentBlock::Text(Arc::from(text))],
        }
    }

    /// 一次工具往返（请求块 + 结果块）打包在一条 `Assistant` 消息里。
    fn assistant_tool_msg(id: u64, call: &str) -> Message {
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

    /// 一段纯聊天的 `Assistant` 回复：没有任何工具结果，所以第 2 档够不着。
    fn assistant_text_msg(id: u64) -> Message {
        Message {
            id: MessageId(id),
            role: Role::Assistant,
            blocks: vec![ContentBlock::Text(Arc::from("一段回答"))],
        }
    }

    /// 5 轮工具会话：每轮一条 `User` + 一条带工具往返的 `Assistant`。
    fn tool_heavy_history() -> Vector<Message> {
        let mut out = Vec::new();
        for turn in 1..=5u64 {
            out.push(user_msg(turn * 2 - 1, "问一句"));
            out.push(assistant_tool_msg(turn * 2, &format!("call_{turn}")));
        }
        Vector::from(out)
    }

    /// 5 轮纯聊天：一个工具结果都没有，第 2 档永远返回空。
    fn chat_heavy_history() -> Vector<Message> {
        let mut out = Vec::new();
        for turn in 1..=5u64 {
            out.push(user_msg(turn * 2 - 1, "聊一句"));
            out.push(assistant_text_msg(turn * 2));
        }
        Vector::from(out)
    }

    fn params() -> ClearParams {
        ClearParams::default()
    }

    /// 压力没超：两档都不开火，哪怕有一大把够得着的工具结果。
    /// **反向锁**——漏了这条会变成每轮改中段、每轮全价，而测试全绿。
    #[test]
    fn below_the_trigger_line_neither_tier_fires() {
        let history = tool_heavy_history();
        let plan = SendPlan::new();
        for prompt in [0, 50, 84, 85] {
            let action = next_action(&history, &plan, Some(prompt), Some(100), params());
            assert_eq!(action, LadderAction::Nothing, "prompt={prompt} 不该开火");
        }
    }

    /// `context_window: None` / 首轮没有实测值：两档都不触发，不 panic。
    #[test]
    fn an_unknown_window_or_a_first_turn_fires_nothing() {
        let history = tool_heavy_history();
        let plan = SendPlan::new();
        assert_eq!(
            next_action(&history, &plan, Some(u32::MAX), None, params()),
            LadderAction::Nothing
        );
        assert_eq!(
            next_action(&history, &plan, None, Some(1000), params()),
            LadderAction::Nothing
        );
    }

    /// 超了且有够得着的工具结果 → **第 2 档**，不是第 3 档。最近 3 轮不动，
    /// 所以前两轮的两个 id 按最老优先出场。
    #[test]
    fn tier_two_wins_whenever_anything_is_still_clearable() {
        let action = next_action(
            &tool_heavy_history(),
            &SendPlan::new(),
            Some(900),
            Some(1000),
            params(),
        );
        let expected = vec![ToolCallId::new("call_1"), ToolCallId::new("call_2")];
        assert_eq!(action, LadderAction::ClearToolResults(expected));
    }

    /// 第 2 档清光之后（同一份历史、同样的压力）才轮到第 3 档，且 `upto` 正好是
    /// 保护区起点——这是「跨轮」那条链在纯函数一侧的形状。
    #[test]
    fn tier_three_takes_over_only_after_tier_two_ran_dry() {
        let history = tool_heavy_history();
        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_1"), ToolCallId::new("call_2")]);

        let action = next_action(&history, &plan, Some(900), Some(1000), params());
        // 5 轮 × 2 条消息，最近 3 轮从第 3 轮的 `User`（下标 4）起。
        assert_eq!(action, LadderAction::Summarize { upto: 4 });
    }

    /// 聊天重的会话：第 2 档从头到尾够不着任何东西，第一次开火就是第 3 档。
    /// （096 决策记录那张表的下面一行。）
    #[test]
    fn a_chat_heavy_session_goes_straight_to_tier_three() {
        let action = next_action(
            &chat_heavy_history(),
            &SendPlan::new(),
            Some(900),
            Some(1000),
            params(),
        );
        assert_eq!(action, LadderAction::Summarize { upto: 4 });
    }

    /// 保护区吃掉整个历史（`User` 消息不足 N 条）：没东西可摘 → `Nothing`，
    /// 不是 `Summarize { upto: 0 }`（摘一段空的会让边界白白前进）。
    #[test]
    fn nothing_to_summarize_is_nothing_not_an_empty_summary() {
        let history = Vector::from(vec![user_msg(1, "只有一轮"), assistant_text_msg(2)]);
        let action = next_action(&history, &SendPlan::new(), Some(999), Some(1000), params());
        assert_eq!(action, LadderAction::Nothing);
        assert_eq!(
            next_action(&Vector::new(), &SendPlan::new(), Some(999), Some(1000), params()),
            LadderAction::Nothing
        );
    }

    /// 红线 1：同一份入参算 1000 次，决定逐字节相同（含清了哪些 id、顺序）。
    #[test]
    fn the_same_inputs_always_yield_the_same_decision() {
        let history = tool_heavy_history();
        let plan = SendPlan::new();
        let first = next_action(&history, &plan, Some(900), Some(1000), params());
        for _ in 0..1000 {
            assert_eq!(
                next_action(&history, &plan, Some(900), Some(1000), params()),
                first
            );
        }
    }
}
