//! [`tool_results_to_clear`]：第 2 档的策略——这一轮该清哪些工具结果（102）。
//!
//! **纯函数**（红线 1）：零 IO、零时钟、零随机、不读全局，只看四个入参。它不写
//! 状态——把选中的 id 真正标记为已清是 101 的 `Session::clear_tool_results`；
//! 这里只回答「清哪些」。
//!
//! **只做算术**（红线 12）：触发只比较窗口压力的百分比，不看 provider、不看
//! 能力位——096 决策 17 已经把「DeepSeek 上该压得更狠」这条路堵死。
//!
//! **触发**：只用上一轮实测的 `TokenUsage.prompt`（滞后一轮，本轮爆掉由 098 兜底）。
//! `last_prompt_tokens` / `context_window` 任一 `None` → 不触发，不 `unwrap`。
//!
//! **一次全清，没有目标水位**：触发之后，保护区之外、还没在 `plan.cleared()`
//! 里的 `ToolResult` 全部返回——够得着的清完为止（096 决策记录第三问）。
//!
//! **保护区按「轮」不按时间**：那条线画在哪由
//! [`protected_region_start`](super::protected_region) 一处回答（第 3 档拿的是同
//! 一条线，所以它不住在这个文件里）。用户消息永不进本档：这里只挑
//! `ContentBlock::ToolResult`，`Role::User` 消息天然不含这种块。

use std::collections::BTreeSet;

use imbl::Vector;

use crate::ids::ToolCallId;
use crate::value::message::{ContentBlock, Message};
use crate::value::send_plan::SendPlan;

use super::pressure::over_trigger_line;
use super::protected_region::protected_region_start;

/// 触发线默认值：85%。低于它第 2 档一次都不开火（反向锁，102 验收）。
/// 数值待 110（真机 dogfood）校准，目前是初始猜测（096 决策记录）。
pub const DEFAULT_TRIGGER_PERCENT: u32 = 85;

/// 保护区默认值：最近 3 轮的工具结果一个不动——清了近期上下文等于自断手脚，
/// 模型会开始重复已经做过的事（096 §四）。
pub const DEFAULT_PROTECT_RECENT_TURNS: usize = 3;

/// [`tool_results_to_clear`] 的两个可配置量。两个值都可配置，默认值理由见上面
/// 两个常量的文档（照 004 `DEFAULT_TOOL_OUTPUT_BYTES` 的做法）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClearParams {
    /// 触发线：`last_prompt_tokens * 100 / context_window` **超过**它才开火。
    /// 恰好等于不触发——边界要有确定的一边。
    pub trigger_percent: u32,
    /// 保护区：最近这么多**轮**的工具结果一个不动。
    pub protect_recent_turns: usize,
}

impl Default for ClearParams {
    fn default() -> Self {
        Self {
            trigger_percent: DEFAULT_TRIGGER_PERCENT,
            protect_recent_turns: DEFAULT_PROTECT_RECENT_TURNS,
        }
    }
}

/// 第 2 档的策略：这一轮该清哪些工具结果。
///
/// **纯函数**（红线 1）：同一份入参算一千次，输出逐项相同（顺序也相同）。
///
/// 返回空 `Vec` 表示「这轮不清」——不触发和「触发了但没东西可清」对调用方是
/// 同一件事（101 的 `clear_tool_results` 对空输入天然无操作）。
///
/// 输出顺序＝在历史中出现的先后（最老在前）；已经在 `plan.cleared()` 里的不再
/// 返回（**单调**——常开会让「最近 N 轮」这条线每轮往前挪一格，每轮都有新的
/// 工具结果被清，等于每轮改中段、每轮全价重编码，102 注意里专门点了这条）。
pub fn tool_results_to_clear(
    history: &Vector<Message>,
    plan: &SendPlan,
    last_prompt_tokens: Option<u32>,
    context_window: Option<u32>,
    params: ClearParams,
) -> Vec<ToolCallId> {
    if !over_trigger_line(last_prompt_tokens, context_window, params.trigger_percent) {
        return Vec::new();
    }

    let protected_from = protected_region_start(history, params.protect_recent_turns);
    let already_cleared: BTreeSet<&ToolCallId> = plan.cleared().iter().collect();

    history
        .iter()
        .take(protected_from)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { id, .. } if !already_cleared.contains(id) => {
                Some(id.clone())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::ids::MessageId;
    use crate::value::message::Role;

    use super::*;

    fn user_msg(id: u64, text: &str) -> Message {
        Message {
            id: MessageId(id),
            role: Role::User,
            blocks: vec![ContentBlock::Text(Arc::from(text))],
        }
    }

    /// 一次工具往返（请求块 + 结果块）打包在一条 `Assistant` 消息里，跟
    /// `send_plan::project` 测试用的 fixture 同一个形状。
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

    /// 4 轮：每轮一条 `User` + 一条带工具往返的 `Assistant`。
    fn four_turn_history() -> Vector<Message> {
        Vector::from(vec![
            user_msg(1, "第一轮"),
            assistant_tool_msg(2, "call_1"),
            user_msg(3, "第二轮"),
            assistant_tool_msg(4, "call_2"),
            user_msg(5, "第三轮"),
            assistant_tool_msg(6, "call_3"),
            user_msg(7, "第四轮"),
            assistant_tool_msg(8, "call_4"),
        ])
    }

    fn params(trigger_percent: u32, protect_recent_turns: usize) -> ClearParams {
        ClearParams {
            trigger_percent,
            protect_recent_turns,
        }
    }

    /// 薄壳：多数测试不需要自定义 `plan`，固定成 `SendPlan::new()`；
    /// `already_cleared_ids_*` 需要自定义 `plan`，直接调原函数。
    fn clear(
        history: &Vector<Message>,
        prompt: Option<u32>,
        window: Option<u32>,
        p: ClearParams,
    ) -> Vec<ToolCallId> {
        tool_results_to_clear(history, &SendPlan::new(), prompt, window, p)
    }

    /// 缺失或无意义的输入一律不触发，不 panic：`context_window` 未知、首轮没有
    /// 实测值、窗口配成 `0`（会导致除零，必须挡住）。
    #[test]
    fn missing_or_degenerate_inputs_never_trigger() {
        let history = four_turn_history();
        let cases: [(Option<u32>, Option<u32>); 3] = [
            (Some(u32::MAX), None),
            (None, Some(1000)),
            (Some(1), Some(0)),
        ];
        for (prompt, window) in cases {
            let out = clear(&history, prompt, window, ClearParams::default());
            assert!(out.is_empty(), "{prompt:?}/{window:?} 不该触发");
        }
    }

    /// 反向锁：用量在触发线以下（含恰好等于）一次都不触发，哪怕有大量够得着的
    /// 工具结果——常开会让每轮都改中段，测试全绿账单上才浮出来。
    #[test]
    fn below_or_at_trigger_line_never_fires() {
        let history = four_turn_history();
        let p = params(85, 1);
        for prompt in [0, 1, 50, 84, 85] {
            assert!(
                clear(&history, Some(prompt), Some(100), p).is_empty(),
                "prompt={prompt} 不该触发"
            );
        }
    }

    /// 超过触发线：保护区之外一次全清，保护区之内（最近 3 轮）一个不动。
    #[test]
    fn above_trigger_line_clears_everything_outside_the_protected_turns() {
        let history = four_turn_history();
        let out = clear(&history, Some(900), Some(1000), params(85, 3));
        assert_eq!(out, vec![ToolCallId::new("call_1")]);
    }

    /// `User` 消息不足 `protect_recent_turns` 条：整个历史都在保护区，不清。
    #[test]
    fn insufficient_user_turns_protects_the_whole_history() {
        let history = four_turn_history();
        let out = clear(&history, Some(999), Some(1000), params(0, 10));
        assert!(out.is_empty());
    }

    /// 单调：已经在 `plan.cleared()` 里的不再返回，哪怕预算比之前更宽松。
    #[test]
    fn already_cleared_ids_never_come_back() {
        let history = four_turn_history();
        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_1"), ToolCallId::new("call_2")]);

        let out = tool_results_to_clear(&history, &plan, Some(900), Some(1000), params(50, 1));
        assert_eq!(out, vec![ToolCallId::new("call_3")]);
    }

    /// 排序：按在历史中出现的先后，最老在前；`protect_recent_turns == 0` 时
    /// 没有保护区，只要触发就连最后一轮也一起清。
    #[test]
    fn output_is_ordered_oldest_first_and_zero_protection_clears_everything() {
        let history = four_turn_history();
        let ids = |n: usize| {
            (1..=n)
                .map(|i| ToolCallId::new(format!("call_{i}")))
                .collect::<Vec<_>>()
        };

        assert_eq!(clear(&history, Some(1), Some(1), params(0, 1)), ids(3));
        assert_eq!(clear(&history, Some(1), Some(1), params(0, 0)), ids(4));
    }

    /// 红线 1：同一份入参算 1000 次，输出逐字节相同（顺序也相同）。
    #[test]
    fn deterministic_across_a_thousand_calls() {
        let history = four_turn_history();
        let p = ClearParams::default();
        let first = clear(&history, Some(900), Some(1000), p);
        for _ in 0..1000 {
            assert_eq!(clear(&history, Some(900), Some(1000), p), first);
        }
    }

    /// 空历史：不 panic，返回空。
    #[test]
    fn empty_history_returns_empty() {
        let out = clear(
            &Vector::new(),
            Some(900),
            Some(1000),
            ClearParams::default(),
        );
        assert!(out.is_empty());
    }
}
