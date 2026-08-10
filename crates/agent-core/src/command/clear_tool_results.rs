//! [`Session::clear_tool_results`]：把一批工具调用的**结果**标记为「不再发送」
//! （101，M12 压缩主干第 2 档的写路径）。只做写路径——「什么时候清、清谁」是
//! 102 的事，这里只交付「一个正规的、能撤的、带记账的清除命令」。
//!
//! ## 这一层补的是值层做不了的那部分
//!
//! 099 的 [`SendPlan::clear_tool_results`](crate::value::send_plan::SendPlan::clear_tool_results)
//! 只管保序去重地把 id 塞进私有 `Vec`——它是纯值，不持有历史，不知道「这个
//! id 在不在这个 agent 的历史里」。这一层补上存在性校验：**不存在的 id 忽略
//! 并计入 [`ClearOutcome::unknown`]，不是拒绝整批**（一批里混一个坏 id 不该
//! 连累其余的）——静默地什么都不做才危险，那会让 102 未来算错的一个 id 藏进
//! 永远不生效也永远不报错的角落；摆进 `unknown` 让调用方能看见、能断言，是
//! 「不静默成功」的落点。已经在列表里的 id 单独计入 `already_cleared`，跟
//! `newly_cleared` 分开，调用方需要分得清「这次到底改了什么」。
//!
//! ## 没有新东西写就不碰 `replace_send_plan`
//!
//! 不是靠它自带的「值相等不落 entry」兜底（虽然那条也成立）——校验完之后
//! `newly_cleared` 空就直接不调用底层 setter，跟 104 `advance_boundary`
//! 「幂等无操作不跑一遍再让 `PartialEq` 吞掉」同一条理由：少一次克隆，也让
//! 「这次调用有没有真的动状态」在代码里是显式分支。生效那一支底层复用
//! `replace_send_plan`（同 104 的理由，见 [`advance_boundary`](super::advance_boundary)
//! 模块文档）：产生的 entry 天然还是 `"replace_send_plan"` 标签，不在
//! `known_label` 的封闭集合里另开一格。

use std::collections::BTreeSet;

use crate::ids::{AgentId, ToolCallId};
use crate::value::message::ContentBlock;

use super::session::Session;

/// 一次 [`Session::clear_tool_results`] 的记账。三个桶互不相交，并集 = 入参
/// 去重后的集合——同一个 id 出现多次只按第一次归属落一个桶。不加 `#[must_use]`：
/// 清除已经真实发生（或如实反映没发生），漏取返回值顶多是没看见记账，照
/// `History::take_drop_events` 先例。
#[derive(Clone, PartialEq, Debug)]
pub struct ClearOutcome {
    /// 本次真正新加进已清列表的。
    pub newly_cleared: Vec<ToolCallId>,
    /// 已经在列表里，幂等跳过的。
    pub already_cleared: Vec<ToolCallId>,
    /// 这个 agent 的历史里找不到对应 `ToolResult`，已忽略的。
    pub unknown: Vec<ToolCallId>,
}

impl Session {
    /// 第 2 档：把这批工具调用的**结果**标记为「不再发送」。
    ///
    /// 内容一个字节不删——完整记录在 `Slot::Messages` 里原样躺着（095 §2），
    /// 这里改的只是 `SendPlan` 的已清列表（发不发）。
    pub fn clear_tool_results(
        &mut self,
        agent: &AgentId,
        ids: impl IntoIterator<Item = ToolCallId>,
    ) -> ClearOutcome {
        let existing = self.tool_result_ids_of(agent);
        let mut plan = self.send_plan_of(agent);
        let already_in_plan: BTreeSet<ToolCallId> = plan.cleared().iter().cloned().collect();

        let mut newly_cleared = Vec::new();
        let mut already_cleared = Vec::new();
        let mut unknown = Vec::new();
        let mut seen = BTreeSet::new();

        for id in ids {
            if !seen.insert(id.clone()) {
                // 这次调用的入参里自己就有重复：只在第一次出现时判定归属，
                // 后面几次原样跳过（不重复计入任何一个桶）。
                continue;
            }
            if !existing.contains(&id) {
                unknown.push(id);
            } else if already_in_plan.contains(&id) {
                already_cleared.push(id);
            } else {
                newly_cleared.push(id);
            }
        }

        if !newly_cleared.is_empty() {
            plan.clear_tool_results(newly_cleared.clone());
            self.replace_send_plan(agent, plan);
        }

        ClearOutcome {
            newly_cleared,
            already_cleared,
            unknown,
        }
    }

    /// 这个 agent 历史里真实出现过的 `ToolResult` id（101「定死的接口」的校验用）。
    fn tool_result_ids_of(&self, agent: &AgentId) -> BTreeSet<ToolCallId> {
        self.messages_of(agent)
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::engine::Event;
    use crate::seam::PrefixImage;
    use crate::value::send_plan::{CLEARED_TOOL_RESULT, project};
    use crate::value::session::{StopReason, TokenUsage};

    use super::*;

    /// 一条用户消息 → 一次带 `n` 个 `ToolUse` 的 `ProviderDone` → `n` 个
    /// `ToolResult` 回填。同 `command/barrier.rs` 的纪律：造状态只走真实事件。
    fn session_with_tool_results(n: usize) -> (Session, Vec<ToolCallId>) {
        let root = AgentId::root();
        let mut session = Session::new(root.clone());
        let _ = session.step(Event::UserInput {
            agent: root.clone(),
            text: Arc::from("跑一批工具"),
        });

        let ids: Vec<ToolCallId> = (0..n).map(|i| ToolCallId::new(format!("call_{i}"))).collect();
        let blocks = ids
            .iter()
            .map(|id| ContentBlock::ToolUse {
                id: id.clone(),
                name: Arc::from("srv:fs/read"),
                input: Arc::new(serde_json::json!({})),
            })
            .collect();
        let _ = session.step(Event::ProviderDone {
            agent: root.clone(),
            epoch: session.epoch(),
            blocks,
            stop: StopReason::ToolUse,
            usage: TokenUsage {
                prompt: 10,
                completion: 5,
                cached: None,
            },
            prefix: PrefixImage {
                segments: Vec::new(),
                prompt_tokens: None,
            },
            adjustments: Vec::new(),
        });

        for id in &ids {
            let _ = session.step(Event::ToolResult {
                agent: root.clone(),
                epoch: session.epoch(),
                call_id: id.clone(),
                content: Arc::from("一大段工具输出"),
            });
        }

        (session, ids)
    }

    /// `project` 输出里，`ids` 对应的 `ToolResult.content` 是否都变成了占位。
    fn assert_masked(projected: &[crate::value::message::Message], ids: &[ToolCallId], masked: bool) {
        let mut checked = 0;
        for msg in projected {
            for block in &msg.blocks {
                if let ContentBlock::ToolResult { id, content, .. } = block
                    && ids.contains(id)
                {
                    checked += 1;
                    if masked {
                        assert_eq!(content.as_ref(), CLEARED_TOOL_RESULT);
                    } else {
                        assert_ne!(content.as_ref(), CLEARED_TOOL_RESULT);
                    }
                }
            }
        }
        assert_eq!(checked, ids.len());
    }

    /// 核心验收串联一处：清 50 条 → 一条 entry、`prev` < 1 KB → `project` 里
    /// 50 条被占位 → `/undo` 一次全回来 → `redo` 一次重新消失。
    #[test]
    fn clearing_fifty_results_is_one_journaled_entry_and_fully_undoable() {
        let (mut s, ids) = session_with_tool_results(50);
        let root = AgentId::root();
        let before_history_len = s.history_len();

        let outcome = s.clear_tool_results(&root, ids.clone());
        assert_eq!(outcome.newly_cleared, ids, "50 个全新 id 原样落进 newly_cleared");
        assert!(outcome.already_cleared.is_empty());
        assert!(outcome.unknown.is_empty());
        assert_eq!(
            s.history_len(),
            before_history_len + 1,
            "清 50 条是一条 journaled entry，不是 50 条"
        );

        let entry = s.history().entries().last().expect("刚落了一条 entry");
        assert_eq!(entry.changes.len(), 1, "只改了 SendPlan 这一个槽位");
        let prev_bytes = serde_json::to_vec(&entry.changes[0].prev).unwrap();
        assert!(
            prev_bytes.len() < 1024,
            "prev 序列化 {} 字节，超过 1 KB",
            prev_bytes.len()
        );

        // 下一轮 encode（用 project 模拟）里这 50 条不再出现（换成占位）。
        let plan = s.send_plan_of(&root);
        let projected = project(&s.messages_of(&root), &plan, None);
        assert_masked(&projected, &ids, true);

        let report = s.undo_step();
        assert!(
            matches!(report, crate::command::UndoReport::Applied { .. }),
            "{report:?}"
        );
        assert!(s.send_plan_of(&root).cleared().is_empty(), "50 条全回来");
        let projected = project(&s.messages_of(&root), &s.send_plan_of(&root), None);
        assert_masked(&projected, &ids, false);

        let _ = s.redo_step();
        assert_eq!(s.send_plan_of(&root).cleared(), ids.as_slice(), "redo 一次 50 条重新消失");
        let projected = project(&s.messages_of(&root), &s.send_plan_of(&root), None);
        assert_masked(&projected, &ids, true);
    }

    /// 101 验收「History.entries() 的长度不变，完整记录一条没少」：这里的
    /// 「完整记录」指 `Slot::Messages`（095/099/100「存的」那条链），不是 undo
    /// 日志——journaled 命令天然多一条 entry（上一个测试已量过它多小），那是
    /// 命令自己的记账。这条测试守的是姊妹承诺：清除不删一个字节。
    #[test]
    fn clearing_does_not_touch_the_underlying_message_record() {
        let (mut s, ids) = session_with_tool_results(5);
        let root = AgentId::root();
        let before = s.messages_of(&root);

        let outcome = s.clear_tool_results(&root, ids);
        assert!(!outcome.newly_cleared.is_empty());
        assert_eq!(s.messages_of(&root), before, "清除只改 SendPlan，不改 Messages 槽位");
    }

    /// 已清、幽灵、同批重复三种一起考：三个桶各归各的，幂等重放不留额外痕迹，
    /// 批内重复 id 只落一份。
    #[test]
    fn already_cleared_unknown_and_duplicate_ids_are_bucketed_correctly() {
        let (mut s, ids) = session_with_tool_results(3);
        let root = AgentId::root();
        let ghost = ToolCallId::new("call_ghost");

        let first = s.clear_tool_results(&root, [ids[0].clone()]);
        assert_eq!(first.newly_cleared, vec![ids[0].clone()]);

        // 第二次：ids[0] 已清、ids[1] 全新且批内自己重复一次、ghost 不存在。
        let before = s.history_len();
        let second = s.clear_tool_results(
            &root,
            [ids[0].clone(), ids[1].clone(), ids[1].clone(), ghost.clone()],
        );
        assert_eq!(second.newly_cleared, vec![ids[1].clone()], "批内重复只落一份");
        assert_eq!(second.already_cleared, vec![ids[0].clone()]);
        assert_eq!(second.unknown, vec![ghost.clone()]);
        assert_eq!(s.history_len(), before + 1, "还有新东西要写，落一条 entry");
        assert_eq!(
            s.send_plan_of(&root).cleared(),
            [ids[0].clone(), ids[1].clone()].as_slice()
        );

        // 第三次：原样重放，三个都已知（要么已清、要么幽灵），没有新东西可写。
        let before = s.history_len();
        let plan_before = s.send_plan_of(&root);
        let third = s.clear_tool_results(&root, [ids[0].clone(), ids[1].clone(), ghost]);
        assert!(third.newly_cleared.is_empty());
        assert_eq!(third.already_cleared, vec![ids[0].clone(), ids[1].clone()]);
        assert_eq!(third.unknown.len(), 1);
        assert_eq!(s.history_len(), before, "没有新东西要写，不留幽灵 entry");
        assert_eq!(s.send_plan_of(&root), plan_before, "prev 也不因此变化");
    }

    /// 只清一个不存在的 id：不 panic，也不静默成功——`unknown` 如实报告，
    /// 已清列表一个字都不多，不落 entry。
    #[test]
    fn an_unknown_id_alone_is_a_reported_no_op() {
        let (mut s, _ids) = session_with_tool_results(1);
        let root = AgentId::root();
        let before = s.history_len();

        let outcome = s.clear_tool_results(&root, [ToolCallId::new("call_ghost")]);

        assert!(outcome.newly_cleared.is_empty());
        assert!(outcome.already_cleared.is_empty());
        assert_eq!(outcome.unknown, vec![ToolCallId::new("call_ghost")]);
        assert_eq!(s.history_len(), before, "不静默成功——但也不落一条空 entry");
        assert!(s.send_plan_of(&root).cleared().is_empty());
    }
}
