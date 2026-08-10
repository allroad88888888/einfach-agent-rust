//! [`Session::send_plan_of`] / [`Session::replace_send_plan`]：`Slot::SendPlan`
//! 的读写口（100，M12 压缩主干第二条）。
//!
//! ## 为什么是这两个函数，不多不少
//!
//! 099 定死了接口：`send_plan_of` 是非创建读（同 `active_skill_names` /
//! `host_tools` 的既有先例），`replace_send_plan` 是**低层 setter，不含任何策略**
//! ——谁该被清、什么时候清是 101/102 的事，这里只负责把值整体换掉并进 undo log
//! （红线 2）。「已清列表要保序去重」「边界只能前进、摘要与边界同进同退」这些
//! 校验全部在 `SendPlan` 自己的方法里（099 已经做完），这一层不重复判断，也判断
//! 不了——`SendPlan` 的字段是私有的，这里拿到的已经是一个满足了三条不变量的值。
//!
//! ## 为什么是 per-agent（`commit_as`）不是会话级（`commit`）
//!
//! 跟 `disabled_builtins` / `host_tools` 不同：这个槽位描述的是「**这个 agent**
//! 这一轮发送侧的账本」，不是整个会话的声明。子 agent 有自己的历史，将来也会有
//! 自己的压缩状态——这也是接口从 099 定死时就带 `agent: &AgentId` 参数、而不是像
//! `host_tools()` 那样只读 root 的原因。
//!
//! ## 为什么没有 `Result` 返回、没有活性校验
//!
//! [`skill`](super::skill) 的 `activate_skill` 会先查 `in_session` / `is_live`，
//! 是因为激活/停用本身带着「这个 skill 现在算不算活的」这层业务语义，拒绝是
//! 可预期的一等结果。`replace_send_plan` 没有这层语义——它就是「把值摆进去」这
//! 一件事，校验和拒绝的责任交给调用它的那一层（101/102 在决定要不要压缩之前，
//! 早就已经确认过这个 agent 活着）；在这里再加一层校验只是把同一个判断在两处
//! 各写一遍，而两处判断迟早会有一处漏改。

use crate::graph::{AtomKey, Slot};
use crate::ids::AgentId;
use crate::value::send_plan::SendPlan;
use crate::value::send_plan_codec;

use super::session::Session;

impl Session {
    /// 读 `agent` 当前的发送计划。**默认是 pristine**（[`SendPlan::new()`]）
    /// ——没写过的 agent 落到 `Slot::default_value()`，编码的正是这个恒等元
    /// （`graph::slot` 的裁决）。
    ///
    /// 非创建读（`peek`，同 `tools_allowed_of` / `active_skill_names` 的既有
    /// 先例）：探一个不在树上的 id 不该在 family 里留下一个没人写的 atom。
    pub fn send_plan_of(&self, agent: &AgentId) -> SendPlan {
        let key = self.send_plan_key(agent);
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        send_plan_codec::from_value(&value)
    }

    /// 整体换掉 `agent` 的发送计划：一条 `Entry`（label `"replace_send_plan"`）。
    ///
    /// 传一个跟当前值相等的 `plan`（最典型的是从没压缩过的 agent 又传一次
    /// `SendPlan::new()`）是真正的空操作：`record_set` 的 `PartialEq` 判定值没变
    /// 就不产生 `Change`，`History::append` 因此不落条目——跟 `disable_builtins`
    /// 的「空开关不落 entry」同一条纪律。
    pub fn replace_send_plan(&mut self, agent: &AgentId, plan: SendPlan) {
        let key = self.send_plan_key(agent);
        let value = send_plan_codec::to_value(&plan);
        self.commit_as(agent, "replace_send_plan", |txn| txn.set_key(key, value));
    }

    fn send_plan_key(&self, agent: &AgentId) -> AtomKey {
        AtomKey::Agent(agent.clone(), Slot::SendPlan)
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::{SummaryId, ToolCallId};

    use super::*;

    fn session() -> Session {
        Session::new(AgentId::root())
    }

    /// 没写过的 agent 读到 pristine；换一个非 pristine 的值 → 一条 journaled
    /// entry → 读回相等 → undo 一次回到 pristine（「undo 白拿」在这个槽位的落点）。
    #[test]
    fn a_replacement_is_journaled_and_undo_takes_it_back() {
        let mut s = session();
        let root = AgentId::root();
        assert_eq!(s.send_plan_of(&root), SendPlan::new(), "全新会话默认 pristine");

        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_1")]);
        plan.advance_boundary(3, Some(SummaryId::new("s1")))
            .unwrap();

        let before = s.history_len();
        s.replace_send_plan(&root, plan.clone());
        assert_eq!(s.history_len(), before + 1, "整体替换是一条 journaled entry");
        assert_eq!(s.send_plan_of(&root), plan);

        let report = s.undo_step();
        assert!(
            matches!(report, crate::command::UndoReport::Applied { .. }),
            "{report:?}"
        );
        assert_eq!(
            s.send_plan_of(&root),
            SendPlan::new(),
            "undo 越过替换那一步之后，应该回到没压缩过的样子"
        );

        let _ = s.redo_step();
        assert_eq!(s.send_plan_of(&root), plan);
    }

    /// 传一个跟当前值相等的 `SendPlan`（pristine 传 pristine）不落 entry——
    /// 跟 `disable_builtins` / `declare_host_tools` 的空写入同一条纪律。
    #[test]
    fn replacing_with_an_equal_plan_leaves_no_trace() {
        let mut s = session();
        let root = AgentId::root();
        let before = s.history_len();
        s.replace_send_plan(&root, SendPlan::new());
        assert_eq!(s.history_len(), before, "值没变，不该留下一条幽灵 entry");
        assert_eq!(s.send_plan_of(&root), SendPlan::new());
    }

    /// 每个 agent 各自一份：子 agent 的替换不影响 root 读到的值。
    #[test]
    fn each_agent_keeps_its_own_plan() {
        let mut s = session();
        let root = AgentId::root();
        let child = s
            .spawn_child(&root, crate::command::ChildConfig::default())
            .expect("spawn 一个子 agent");

        let mut plan = SendPlan::new();
        plan.advance_boundary(1, None).unwrap();
        s.replace_send_plan(&child, plan.clone());

        assert_eq!(s.send_plan_of(&child), plan);
        assert_eq!(
            s.send_plan_of(&root),
            SendPlan::new(),
            "子 agent 的替换不该漏到 root 头上"
        );
    }
}
