//! [`Session::apply_summary`] / [`Session::summary_text`]：摘要回来了，把它写进状态
//! （107，M12 压缩主干第 3 档的最后一步）。
//!
//! ## 一条 entry 同时做三件事
//!
//! 存正文（`Slot::Summaries`）、推边界、填引用（后两件是 `Slot::SendPlan` 的同一次
//! 写入）。三件事必须原子：**拆开会出现「边界推了但还没有摘要」的一瞬间，那时的
//! prompt 缺一整段**——投影拿不到正文会把边界作废（099「宁可多发，不可发空洞」）；
//! 反过来先存正文再推边界，中间那一刻库里多一份没人引用的摘要，undo 的步数也对
//! 不上。所以这里**不调** [`advance_boundary`](Session::advance_boundary)（104）：
//! 那条命令自带一次 `replace_send_plan`，也就自带一条 `Entry`。分诊表跟它同一张，
//! 写入这一步自己做——一次 `commit_as` 里写两个逻辑键，落成一条 `Entry`。
//!
//! ## `SummaryId` 从 `upto` 派生，不用计数器
//!
//! **一个边界值最多对应一个摘要**：104 已经把「同边界换摘要」定成拒绝
//! （[`BoundaryRejected::SameBoundaryDifferentSummary`]），边界又只增不减，所以
//! `upto` 本身就是唯一键。于是不需要计数器槽位，也**不需要任何随机或时钟**
//! （红线 1）——同一份历史重放两次，摘要 id 逐字节相同。一处措辞变化：「同边界换
//! 摘要」不再表现为 id 不同（id 必然相同），而是表现为**正文不同**；判定点从比 id
//! 挪到比正文，拒绝的语义一个字没变。
//!
//! ## epoch 在哪校验（红线 6）
//!
//! **不在这里。** 这是一条命令，跟 `advance_boundary` / `clear_tool_results` 一样
//! 表达「此刻的意图」；红线 6 的闸装在 [`step`](super::step) 入口：在飞的摘要回执是
//! `Event::CompactDone`，它带 `epoch`，过不了闸整条丢弃——不写、不报错、不重试
//! （105 落地）。闸只有一处是刻意的（`step` 的文档：转移表有几十格，漏一格就是漏
//! 一条回写路径，而漏了不报错）。所以 [`BoundaryRejected`] 两个变体都是边界语义上
//! 的拒绝，没有、也不该有「这份摘要过期了」那一种。
//!
//! 由此得到留给下游（108 接阶梯的人）的**硬契约**：持有 `upto` 的那一方必须先把
//! `Event::CompactDone` 喂给 `Session::step`，**只有过了闸**（回执里有
//! `Notice::CompactionSummaryReceived`——105 专门为了让「接受」可观测才加的那一条）
//! 才调 `apply_summary`。绕开 `step` 直接调，就是把一份属于旧世代的摘要盖到当前
//! 历史上：边界推到 `upto`，而那段历史已经被 undo 掉了——下一轮 prompt 少一整段，
//! 模型照答不误，人发现不了（红线 6 原话：「在 undo 或崩溃恢复时以静默错值的形式
//! 浮出来」）。`upto` 不在事件里是 105 定死的形状（effect 不带历史正文，事件也没有
//! 理由胖），这条契约因此守不进类型系统，只能由 108 的接线兑现。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::ids::{AgentId, SummaryId};
use crate::value::{send_plan_codec, summaries};

use super::advance_boundary::BoundaryRejected;
use super::session::Session;

impl Session {
    /// 摘要回来了：**一条 entry 同时做三件事**——存正文、推边界、填引用。
    /// `SummaryId` 由 `upto` 派生，不需要调用方给（见模块文档）。
    ///
    /// 三种情况，跟 [`advance_boundary`](Session::advance_boundary) 同一张分诊表：
    /// `upto >` 当前边界 → 生效，产生**一条** entry；`upto ==` 当前边界且已经生效
    /// 的就是这份摘要（id 与正文都相同）→ **幂等无操作，不产生 entry**；其余 →
    /// `Err`，**状态与日志都不变**（先校验再写，拒绝路径不留痕）。
    ///
    /// ⚠️ 调用前先过 `Session::step` 的 epoch 闸，理由见模块文档「epoch 在哪校验」。
    pub fn apply_summary(
        &mut self,
        agent: &AgentId,
        upto: usize,
        summary: Arc<str>,
    ) -> Result<SummaryId, BoundaryRejected> {
        let id = summary_id_for(upto);
        let mut plan = self.send_plan_of(agent);
        let current = plan.boundary();

        if upto < current {
            return Err(BoundaryRejected::NotAdvancing {
                current,
                requested: upto,
            });
        }

        let mut library = self.summaries_of(agent);
        let stored = library
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, text)| text.clone());

        // 「一个边界值最多对应一个摘要」。同一个 `upto` 再来一份**不同**正文，就是
        // 104 定成拒绝的那件事换了个入口（见模块文档）——在这里挡住，库里因此不会
        // 出现两条同 id 的记录，`summary_text` 的查表也就不必回答「取哪一条」。
        if stored
            .as_deref()
            .is_some_and(|text| text != summary.as_ref())
        {
            return Err(BoundaryRejected::SameBoundaryDifferentSummary);
        }

        if upto == current {
            // 边界没动：唯一能接受的是「跟已经生效的那一份逐字相同」，那是幂等，
            // 不落 entry（同 104 的第二种情况）。引用没指向它（比如第 4 档刚清过
            // 窗口、`summary` 是 `None`）就是拒绝——「重新摘要同一段」不在支持范围
            // 内，那是一条新决策。
            return if plan.summary() == Some(&id) && stored.is_some() {
                Ok(id)
            } else {
                Err(BoundaryRejected::SameBoundaryDifferentSummary)
            };
        }

        // upto > current：三件事，一条 entry。值层的不变量在这里必然满足，
        // `expect` 是把「校验已经在上面做完」写进类型里，不是抱侥幸心理。
        plan.advance_boundary(upto, Some(id.clone()))
            .expect("upto > current 已在上面校验过，值层不会再拒绝");
        if stored.is_none() {
            // **只增不删**：前几次压缩的摘要留在库里（redo 要取得回正文）。
            library.push((id.clone(), summary));
        }

        let summaries_key = AtomKey::Agent(agent.clone(), Slot::Summaries);
        let summaries_value = summaries::to_value(&library);
        let plan_key = AtomKey::Agent(agent.clone(), Slot::SendPlan);
        let plan_value = send_plan_codec::to_value(&plan);
        self.commit_as(agent, "apply_summary", |txn| {
            txn.set_key(summaries_key, summaries_value);
            txn.set_key(plan_key, plan_value);
        });
        Ok(id)
    }

    /// 取某个摘要的正文。投影（099 的 `project`）要用它。
    ///
    /// 找不到 → `None`，投影那边会把边界作废（宁可多发，不可发空洞）。
    pub fn summary_text(&self, agent: &AgentId, id: &SummaryId) -> Option<Arc<str>> {
        self.summaries_of(agent)
            .into_iter()
            .find(|(known, _)| known == id)
            .map(|(_, text)| text)
    }

    /// 这个 agent 的整份摘要库。非创建读（`peek`，同 `send_plan_of` 的先例）：
    /// 探一个不在树上的 id 不该在 family 里留下一个没人写的 atom。
    fn summaries_of(&self, agent: &AgentId) -> Vec<(SummaryId, Arc<str>)> {
        let key = AtomKey::Agent(agent.clone(), Slot::Summaries);
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        summaries::from_value(&value)
    }
}

/// `SummaryId` = 这次摘要盖住的边界值。见模块文档「从 `upto` 派生」。
///
/// **没有时钟、没有随机、没有计数器**（红线 1）：同一份历史重放两次，
/// 摘出来的 id 逐字节相同，审计回放才对得上。
fn summary_id_for(upto: usize) -> SummaryId {
    SummaryId::new(format!("summary@{upto}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Slot;

    fn session() -> Session {
        Session::new(AgentId::root())
    }

    /// 三件事一条 entry：正文进库、边界推进、引用填上，**一条 `Entry` 两个槽位**。
    #[test]
    fn one_entry_stores_the_text_moves_the_boundary_and_fills_the_reference() {
        let mut s = session();
        let root = AgentId::root();
        let before = s.history_len();

        let id = s.apply_summary(&root, 5, Arc::from("前五条摘要")).unwrap();

        assert_eq!(s.history_len(), before + 1, "三件事只该落一条 entry");
        let entry = s.history().entries().last().expect("刚落了一条 entry");
        let touched: Vec<&AtomKey> = entry.changes.iter().map(|c| &c.key).collect();
        assert_eq!(touched.len(), 2, "这一条 entry 恰好动两个槽位");
        for slot in [Slot::Summaries, Slot::SendPlan] {
            let key = AtomKey::Agent(root.clone(), slot);
            assert!(touched.contains(&&key), "{slot:?} 该在同一条 entry 里");
        }
        let plan = s.send_plan_of(&root);
        assert_eq!(plan.boundary(), 5);
        assert_eq!(plan.summary(), Some(&id));
        assert_eq!(s.summary_text(&root, &id).as_deref(), Some("前五条摘要"));
    }

    /// `/undo` 一次，三件事一起退回——它们本来就是一条 entry。
    #[test]
    fn undo_takes_all_three_back_at_once() {
        let mut s = session();
        let root = AgentId::root();
        let id = s.apply_summary(&root, 5, Arc::from("摘要正文")).unwrap();

        let report = s.undo_step();
        let applied = matches!(report, crate::command::UndoReport::Applied { .. });
        assert!(applied, "{report:?}");
        let pristine = crate::value::send_plan::SendPlan::new();
        assert_eq!(s.send_plan_of(&root), pristine, "边界与引用一起退回");
        assert_eq!(s.summary_text(&root, &id), None, "正文也退回，不留在库里");

        let _ = s.redo_step();
        assert_eq!(s.send_plan_of(&root).boundary(), 5);
        assert_eq!(s.summary_text(&root, &id).as_deref(), Some("摘要正文"));
    }

    /// id 由 `upto` 派生：两份互不相干的会话、不同的正文，同一个 `upto` 出同一个
    /// id；不同的 `upto` 出不同的 id。**重放确定**（红线 1）。
    #[test]
    fn the_id_comes_from_upto_alone() {
        let (mut a, mut b, root) = (session(), session(), AgentId::root());
        let ia = a.apply_summary(&root, 7, Arc::from("正文甲")).unwrap();
        let ib = b.apply_summary(&root, 7, Arc::from("正文乙不同")).unwrap();
        assert_eq!(ia, ib, "同一个 upto 必须派生出同一个 id");
        let later = a.apply_summary(&root, 9, Arc::from("后一份")).unwrap();
        assert_ne!(ia, later);
    }

    /// 连续两次压缩：边界继续前进，**第一份摘要留在库里不回收**
    /// （回收了 redo 就拿不回来）。
    #[test]
    fn a_second_compaction_keeps_the_first_summary() {
        let mut s = session();
        let root = AgentId::root();
        let first = s.apply_summary(&root, 3, Arc::from("第一份")).unwrap();
        let second = s.apply_summary(&root, 8, Arc::from("第二份")).unwrap();

        assert_eq!(s.send_plan_of(&root).boundary(), 8);
        assert_eq!(s.send_plan_of(&root).summary(), Some(&second));
        assert_eq!(s.summary_text(&root, &first).as_deref(), Some("第一份"));
        assert_eq!(s.summary_text(&root, &second).as_deref(), Some("第二份"));
    }

    /// 边界后退：拒绝，状态与日志都不动（拒绝路径不留痕）。
    #[test]
    fn a_smaller_upto_is_rejected_and_leaves_no_trace() {
        let mut s = session();
        let root = AgentId::root();
        s.apply_summary(&root, 5, Arc::from("摘要")).unwrap();
        let before_len = s.history_len();
        let before_plan = s.send_plan_of(&root);

        let err = s.apply_summary(&root, 3, Arc::from("更早")).unwrap_err();

        let expected = BoundaryRejected::NotAdvancing {
            current: 5,
            requested: 3,
        };
        assert_eq!(err, expected);
        assert_eq!(s.history_len(), before_len);
        assert_eq!(s.send_plan_of(&root), before_plan);
        assert_eq!(s.summary_text(&root, &summary_id_for(3)), None);
    }

    /// 边界没动时只接受**逐字相同**的那一份：换正文 = 「同边界换摘要」→ 拒绝；
    /// 原样重放 → 幂等。两条都不留痕。第三段是第 4 档清窗口之后一份迟到的、正好
    /// 盖到这个边界的摘要——同样拒绝，「重新摘要同一段」是一条新决策。
    #[test]
    fn the_same_boundary_accepts_only_the_very_same_text() {
        let mut s = session();
        let root = AgentId::root();
        let id = s.apply_summary(&root, 5, Arc::from("原来那份")).unwrap();
        let before_len = s.history_len();
        let before_plan = s.send_plan_of(&root);

        let err = s.apply_summary(&root, 5, Arc::from("换一份")).unwrap_err();
        assert_eq!(err, BoundaryRejected::SameBoundaryDifferentSummary);
        assert_eq!(s.history_len(), before_len, "拒绝不该留下一条 entry");
        assert_eq!(s.send_plan_of(&root), before_plan);
        assert_eq!(s.summary_text(&root, &id).as_deref(), Some("原来那份"));

        let same = s.apply_summary(&root, 5, Arc::from("原来那份")).unwrap();
        assert_eq!(same, id, "原样重放是幂等");
        assert_eq!(s.history_len(), before_len, "幂等无操作不产生 entry");

        let mut win = session();
        win.advance_boundary(&root, 6, None).unwrap();
        let len = win.history_len();
        let err = win.apply_summary(&root, 6, Arc::from("迟到")).unwrap_err();
        assert_eq!(err, BoundaryRejected::SameBoundaryDifferentSummary);
        assert_eq!(win.history_len(), len);
        assert_eq!(win.send_plan_of(&root).summary(), None);
    }

    /// 红线 5 与 095 形状决策的兑现点：这条 entry 的 `prev` **大小与摘要正文长度
    /// 无关**——摘要 100 字节和 10 KB，第一次压缩的 `prev` 序列化后一样大
    /// （装的是「压之前的空库」和一个 pristine 计划）。
    #[test]
    fn the_prev_does_not_grow_with_the_summary_text() {
        fn prev_bytes(text: &str) -> usize {
            let (mut s, root) = (session(), AgentId::root());
            s.apply_summary(&root, 5, Arc::from(text)).unwrap();
            let entry = s.history().entries().last().unwrap().clone();
            let sizes = entry.changes.iter().map(|c| serde_json::to_vec(&c.prev));
            sizes.map(|bytes| bytes.unwrap().len()).sum()
        }
        let small = prev_bytes(&"短".repeat(50));
        let large = prev_bytes(&"长".repeat(10_000));
        assert_eq!(small, large, "prev 不该随摘要正文长大");
        assert!(small < 1024, "第一次压缩的 prev 实测 {small} 字节");
    }
}
