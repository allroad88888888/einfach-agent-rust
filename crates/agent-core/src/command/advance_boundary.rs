//! [`Session::advance_boundary`]：把 `SendPlan` 的边界推到 `next`，同时设定/清除
//! 摘要引用（104，M12 压缩主干「第 4 档 + 第 3 档共用的边界推进」）。
//!
//! ## 为什么第 4 档不单开机制
//!
//! 「清窗口」= 边界推到历史长度、摘要引用留空；「用户主动摘要」= 边界推到
//! 「最近 3 轮之前」、摘要引用填上。两者共用同一个字段、同一条命令——区别只在
//! `summary` 传不传 `Some`，不值得为「清窗口」单开一套机制（096 §八）。
//!
//! ## 三种情况，别合并（104「定死的接口」）
//!
//! - `next > 当前边界` → 生效，产生一条 entry
//! - `next == 当前边界` 且摘要引用相同 → **幂等无操作，不产生 entry**
//! - 其余（`next` 更小；或 `next` 等于当前边界但摘要引用不同）→ `Err`，
//!   **状态与 entry 都不变**（先校验再写，拒绝路径不留痕）
//!
//! ## 为什么这一层不能直接把值层的 `Result` 转译一遍完事
//!
//! 099 的 [`SendPlan::advance_boundary`](crate::value::send_plan::SendPlan::advance_boundary)
//! 对 `next <= self.boundary()` 一律 `Err`（`BoundaryNotAdvancing`），把「相等」并
//! 进了「拒绝」——那是值层的判断：它不知道调用方是不是刚巧把上一次算出的边界又
//! 传了一遍。命令层多知道一件事：触发逻辑（096 的自动阶梯、用户按钮）算出来的
//! `next` 等于已经生效的边界，是「这一轮没有新东西可压」，不是 bug。所以这里要
//! 在调用值层方法**之前**先分诊，把「相等」这一种从「拒绝」里挑出来改判成
//! 「幂等无操作」；真正调到值层方法时，`next` 已经保证严格大于当前边界。
//!
//! ## 底层复用 `replace_send_plan`，不新开一个 entry 标签
//!
//! 生效的那一支直接调 [`Session::replace_send_plan`](super::send_plan)（100）整体
//! 换掉 `SendPlan`。entry 的 label 因此还是 `"replace_send_plan"`——这条命令在状态
//! 层做的事就是「整体换掉这个槽位的值」，跟将来 101/102 调同一个底层 setter 产生
//! 的 entry 是同一种事件，没有理由在 `known_label` 的封闭集合里为它再开一格。

use crate::ids::{AgentId, SummaryId};

use super::session::Session;

/// [`Session::advance_boundary`] 被拒的理由。**都是可预期的拒绝，不是 bug**——
/// 同 [`SkillError`](super::SkillError) / [`SpawnRefused`](super::SpawnRefused) 的
/// 定位，调用方据此原样回报或静默重试，不是要修的运行时错误。
#[derive(Clone, PartialEq, Debug)]
pub enum BoundaryRejected {
    /// `next` 比当前边界小——边界只能前进（回退会让 History 段来回漂，每轮全价，
    /// 跟 102 的滞回带是同一类坑）。
    NotAdvancing { current: usize, requested: usize },
    /// 边界没动但摘要引用不同。**重新摘要同一段不在本条支持范围内**——
    /// 若 107 之后需要「摘要重生成」，那是一条新决策，不是这里顺手放开。
    SameBoundaryDifferentSummary,
}

impl Session {
    /// 第 3、4 档共用：把 `agent` 的发送边界推到 `next`，同时设定/清除摘要引用。
    ///
    /// 第 4 档「清窗口」= `next` 取历史长度、`summary` 传 `None`。
    /// 用户主动摘要 = `next` 取「最近 3 轮之前」、`summary` 传 `Some`。
    ///
    /// 两个用户按钮都**不受 X / Y 水位约束**——水位是自动档判「够不够」用的，
    /// 用户按下去就是执行一次（096 §八）。
    pub fn advance_boundary(
        &mut self,
        agent: &AgentId,
        next: usize,
        summary: Option<SummaryId>,
    ) -> Result<(), BoundaryRejected> {
        let mut plan = self.send_plan_of(agent);
        let current = plan.boundary();

        if next < current {
            return Err(BoundaryRejected::NotAdvancing {
                current,
                requested: next,
            });
        }

        if next == current {
            return if plan.summary() == summary.as_ref() {
                // 幂等：调用方算出来的边界跟已经生效的一样，没有新东西要写。
                // 连一次「值相等因此不落 entry」的空写都不必发生——直接不碰
                // `replace_send_plan`，比让它跑一遍再靠 `PartialEq` 吞掉更直接。
                Ok(())
            } else {
                Err(BoundaryRejected::SameBoundaryDifferentSummary)
            };
        }

        // next > current：值层的不变量在这里必然满足。`expect` 是把「校验已经在
        // 上面做完」这件事写进类型里，不是抱侥幸心理。
        plan.advance_boundary(next, summary)
            .expect("next > current 已在上面校验过，值层不会再拒绝");
        self.replace_send_plan(agent, plan);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::SummaryId;

    use super::*;

    fn session() -> Session {
        Session::new(AgentId::root())
    }

    /// `next > 当前边界`：生效，产生一条 entry，边界与摘要引用一起改。
    #[test]
    fn advancing_journals_one_entry_and_moves_both_fields_together() {
        let mut s = session();
        let root = AgentId::root();
        let before = s.history_len();

        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();

        assert_eq!(s.history_len(), before + 1, "生效是一条 journaled entry");
        let plan = s.send_plan_of(&root);
        assert_eq!(plan.boundary(), 5);
        assert_eq!(plan.summary(), Some(&SummaryId::new("s1")));
    }

    /// 第 4 档「清窗口」的字面形状：`summary` 传 `None`，边界照样推进。
    #[test]
    fn clearing_the_window_advances_with_no_summary() {
        let mut s = session();
        let root = AgentId::root();

        s.advance_boundary(&root, 8, None).unwrap();

        let plan = s.send_plan_of(&root);
        assert_eq!(plan.boundary(), 8);
        assert_eq!(plan.summary(), None);
    }

    /// `next == 当前边界` 且摘要相同：幂等无操作，**不产生 entry**——覆盖
    /// pristine（0/None）与非 pristine 两种起点。
    #[test]
    fn same_boundary_and_summary_is_idempotent_and_leaves_no_trace() {
        let mut s = session();
        let root = AgentId::root();

        // pristine 起点：再推一次 0/None。
        let before = s.history_len();
        s.advance_boundary(&root, 0, None).unwrap();
        assert_eq!(s.history_len(), before, "pristine 再推 0/None 不该留痕");

        // 非 pristine 起点：先推到 5/Some(s1)，再原样重放一次。
        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();
        let before = s.history_len();
        let plan_before = s.send_plan_of(&root);

        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();

        assert_eq!(s.history_len(), before, "边界推到底之后再推一次：幂等");
        assert_eq!(s.send_plan_of(&root), plan_before);
    }

    /// `next` 比当前边界小：拒绝，状态与历史都不动，不是静默忽略。
    #[test]
    fn a_smaller_boundary_is_rejected_and_leaves_state_untouched() {
        let mut s = session();
        let root = AgentId::root();
        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();

        let before = s.history_len();
        let plan_before = s.send_plan_of(&root);

        let err = s.advance_boundary(&root, 3, None).unwrap_err();

        assert_eq!(
            err,
            BoundaryRejected::NotAdvancing {
                current: 5,
                requested: 3,
            }
        );
        assert_eq!(s.history_len(), before, "拒绝不该留下一条 entry");
        assert_eq!(s.send_plan_of(&root), plan_before, "拒绝不该动状态");
    }

    /// `next == 当前边界` 但摘要引用不同：拒绝（「重新摘要同一段」不在本条范围），
    /// 同样不留痕。
    #[test]
    fn same_boundary_different_summary_is_rejected_and_leaves_no_trace() {
        let mut s = session();
        let root = AgentId::root();
        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();

        let before = s.history_len();
        let plan_before = s.send_plan_of(&root);

        let err = s
            .advance_boundary(&root, 5, Some(SummaryId::new("s2")))
            .unwrap_err();

        assert_eq!(err, BoundaryRejected::SameBoundaryDifferentSummary);
        assert_eq!(s.history_len(), before);
        assert_eq!(s.send_plan_of(&root), plan_before);
    }

    /// `/undo` 一次把边界退回去——那是 undo 走的 undo log，不是「后退」这条命令
    /// 本身支持的路径。
    #[test]
    fn undo_moves_the_boundary_back() {
        let mut s = session();
        let root = AgentId::root();
        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();
        assert_eq!(s.send_plan_of(&root).boundary(), 5);

        let report = s.undo_step();
        assert!(
            matches!(report, crate::command::UndoReport::Applied { .. }),
            "{report:?}"
        );
        assert_eq!(
            s.send_plan_of(&root),
            crate::value::send_plan::SendPlan::new(),
            "undo 越过这一步之后应该回到没压缩过的样子"
        );

        let _ = s.redo_step();
        assert_eq!(s.send_plan_of(&root).boundary(), 5);
    }

    /// 红线 3 落地检查：这条 entry 的 `prev` 序列化 < 1 KB——它装的是一个数
    /// （旧边界）和一个 id（旧摘要引用），不该随别的东西长大。
    #[test]
    fn the_journaled_entrys_prev_is_small() {
        let mut s = session();
        let root = AgentId::root();
        s.advance_boundary(&root, 5, Some(SummaryId::new("s1")))
            .unwrap();

        let entry = s.history().entries().last().expect("刚落了一条 entry");
        assert_eq!(entry.changes.len(), 1, "只改了 SendPlan 这一个槽位");
        let prev_bytes = serde_json::to_vec(&entry.changes[0].prev).unwrap();
        assert!(
            prev_bytes.len() < 1024,
            "prev 序列化 {} 字节，超过 1 KB",
            prev_bytes.len()
        );
    }
}
