//! [`Session`] 的公开读口：宿主取料的地方（ADAPTER.md §时序——runner 收到
//! `Effect::CallProvider` 之后，在 actor 线程上从状态取料组装 `Ingredients`）。
//!
//! **形状对齐 M1 的 `TurnState` 字段**：runner 原来读 `state.messages` /
//! `state.prev_prefix` / `state.status` / `state.epoch`，现在读同名方法，
//! 返回的是**值的克隆**而不是引用——`store.get()` 本来就是返回 owned 值，而所有
//! 可能变大的东西都在 `Arc` / `imbl::Vector` 后面（红线 5），克隆是指针拷贝。
//!
//! 没有 `store()`：见 [`Session`] 的文档注释。
//!
//! # per-agent 取料口（029）
//!
//! 下面那批 `*_of(agent)` 是宿主替**某一个** agent 组它自己的 `Ingredients` 时
//! 读的口子——028 推给 029 的第 4 条点名要求的东西，也点名了怎么写错：
//! **它不是第三个跨 agent 读 API**。跨 agent 读只有 `read_ancestor` /
//! `read_descendant` 两个（`cross_read.rs`，红线 10 的方向与 `Visibility` 校验
//! 都在那里），本节一条都不碰：宿主替 root 取 root 自己的消息、替子 agent 取
//! 子 agent 自己的消息，读的是「它自己的」槽位，不产生图上的边，也没有「方向」
//! 可校验。不带参数的那批（`messages()` / `status()` / …）从此就是它们在 root
//! 上的特化，**同一条实现**——分成两条的那一刻，root 和子 agent 的取料就会开始
//! 悄悄分叉。

use std::sync::Arc;

use imbl::Vector;

use crate::engine::epoch::Epoch;
use crate::engine::state::{ToolSlot, TurnStatus};
use crate::graph::{AtomKey, DerivedKey, Slot, derived_atom};
use crate::ids::{AgentId, MessageId};
use crate::seam::PrefixImage;
use crate::value::atom_value::AgentValue;
use crate::value::message::Message;

use super::meta::{AgentEntry, AgentHistory};
use super::session::Session;

impl Session {
    /// 这棵树的 **root**。会话级命令（`begin_turn` / `set_max_*`）落在它头上，
    /// `turn_id` 由它铸（决策 5），下面那批不带 agent 参数的读口读的也是它。
    ///
    /// 028 之前这个方法的语义是「这份状态是谁的」，那时一个 `Session` 只有一个
    /// agent，两者恰好重合。现在它装的是整棵树：**「这一步替谁做」不再由这里回答**
    /// ——那是 `Session::step` 从事件的 `agent` 字段路由出来的，effect 里的 `agent`
    /// 也跟着它走。路由权仍然没有交给宿主，闸在 `step` 里（不在活名单上的 agent
    /// 事件直接丢），只是闸的形式从「不看你说的」变成了「看，但要核」。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 当前世代（红线 6）。发 effect 时带上它，结果回写前由 `step` 的闸比对。
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// 当前 turn 号。`undo_turn` 的分组依据，也是 UI 时间线的分段依据。
    pub fn turn_id(&self) -> u64 {
        self.turn_id
    }

    /// 读一个 agent 的一个槽位。**非创建**（走 [`Session::peek`]）：探一个不在
    /// 树上的 id 不该在 family 里留下十个没人写的 atom，还跟着进快照——028 判断 6
    /// 对跨 agent 读口下的同一条判断，per-agent 取料口没有理由更宽松。
    ///
    /// 键不在 family 里就落 [`AtomKey::default_value`]：对活着的 agent 这一支永远
    /// 走不到（`build_agent` 把十个槽位一次建齐），它兜的是「宿主拼错了一个
    /// `AgentId`」——那种情况下拿到一份默认值，比顺手建一棵幽灵树好。
    fn slot_of(&self, agent: &AgentId, slot: Slot) -> AgentValue {
        let key = AtomKey::Agent(agent.clone(), slot);
        self.peek(&key).unwrap_or_else(|| key.default_value())
    }

    fn read(&self, slot: Slot) -> AgentValue {
        self.slot_of(&self.agent, slot)
    }

    pub fn status(&self) -> TurnStatus {
        self.status_of(&self.agent)
    }

    pub fn messages(&self) -> Vector<Message> {
        self.messages_of(&self.agent)
    }

    /// 上一次请求的前缀镜像。**core 只存、只原样传回料单，不判读**——「哪一段漂了」
    /// 要对着新请求的原始字节算，只有 adapter 干得了（红线 12，ADAPTER.md）。
    pub fn prev_prefix(&self) -> Option<PrefixImage> {
        self.prev_prefix_of(&self.agent)
    }

    /// 这个 agent 的轮状态。029 的泵用它判「子 agent 到终态了没」——
    /// 见本文件顶部「per-agent 取料口」。
    pub fn status_of(&self, agent: &AgentId) -> TurnStatus {
        self.slot_of(agent, Slot::Status)
            .as_status()
            .expect("Status 槽位持 Status")
            .clone()
    }

    /// 这个 agent 自己的消息历史（组它自己的 `Ingredients` 用）。
    pub fn messages_of(&self, agent: &AgentId) -> Vector<Message> {
        self.slot_of(agent, Slot::Messages)
            .as_messages()
            .expect("Messages 槽位持 Messages")
            .clone()
    }

    /// 这个 agent 上一次请求的前缀镜像。每个 agent 各自一份——它们是各自独立的
    /// 请求流，拿别人的镜像去比对自己的字节只会把正常的差异误报成漂移。
    pub fn prev_prefix_of(&self, agent: &AgentId) -> Option<PrefixImage> {
        self.slot_of(agent, Slot::PrevPrefix).as_prefix().cloned()
    }

    /// spawn 当时快照的工具子集（`Slot::ToolsAllowed`）。
    ///
    /// `None` = 这个槽位是 `Null`，也就是**这个 agent 不是被 spawn 出来的**——
    /// root 恒是这一支（它的活性来自会话本身），已经 despawn / spawn 被 undo 撤掉
    /// 的子 agent 也是。调用方（029 的宿主）对 root 的解释是「不受子集约束，用宿主
    /// 的整张工具表」，对死掉的 agent 压根不会问（`is_live` 先答了）。
    ///
    /// 顺序原样返回（写入时已经排序去重，红线 11）。
    pub fn tools_allowed_of(&self, agent: &AgentId) -> Option<Vec<Arc<str>>> {
        let value = self.slot_of(agent, Slot::ToolsAllowed);
        let array = value.as_json()?.as_array()?.clone();
        Some(
            array
                .iter()
                .filter_map(|v| v.as_str().map(Arc::from))
                .collect(),
        )
    }

    /// 本轮的工具槽，**顺序就是模型请求的顺序**。
    pub fn tool_slots(&self) -> Arc<Vec<ToolSlot>> {
        self.tool_slots_of(&self.agent.clone())
    }

    /// 这个 agent 自己的工具槽（组它自己的 `Ingredients` 用，同 `tool_slots()`
    /// 对 root 的语义）。046 的 `agent_tree()` 拼 `AgentActivity::Working.tools`
    /// 时读它——在飞工具名是这个槽 `SlotState::Pending` 的那些条目的投影，不是新槽。
    pub fn tool_slots_of(&self, agent: &AgentId) -> Arc<Vec<ToolSlot>> {
        self.slot_of(agent, Slot::ToolSlots)
            .as_slots()
            .expect("ToolSlots 槽位持 Slots")
            .clone()
    }

    /// 收敛判断：**读那个 derived atom**，不是现扫一遍（003 预言的落点在此兑现）。
    ///
    /// 零个槽位返回 `true`（没有东西要等）；有任何一个 `Pending` 返回 `false`。
    /// undo 回滚了槽位之后这个答案**自动**跟着回来——它是图上的一个值，不是某处
    /// 维护出来的计数。
    pub fn tools_converged(&self) -> bool {
        let id = derived_atom(
            &self.store,
            &self.sources,
            &self.derived,
            &DerivedKey::ToolsConverged(self.agent.clone()),
        );
        matches!(self.store.get(id), AgentValue::Bool(true))
    }

    /// 下一个要铸的 `MessageId`。
    pub fn next_message_id(&self) -> MessageId {
        MessageId(
            self.read(Slot::NextMessageId)
                .as_u64()
                .expect("NextMessageId 槽位持 U64"),
        )
    }

    fn count(&self, slot: Slot) -> u32 {
        self.read(slot).as_u64().expect("计数槽位持 U64") as u32
    }

    /// 本轮已经发起的 `CallProvider` 次数——新一轮和重试都算。
    pub fn turns_used(&self) -> u32 {
        self.count(Slot::TurnsUsed)
    }

    pub fn max_turns(&self) -> u32 {
        self.count(Slot::MaxTurns)
    }

    /// 当前这条失败-重试链已经连续失败了几次。
    pub fn retries_used(&self) -> u32 {
        self.count(Slot::RetriesUsed)
    }

    pub fn max_retries(&self) -> u32 {
        self.count(Slot::MaxRetries)
    }

    /// **完整状态**：所有 primitive 的当前值，按逻辑键排序。
    ///
    /// 这就是 010 的 `Snapshot` 形状（`Vec<(AtomKey, Value)>`，只存 primitive）。
    /// 排序不是装饰：两份快照要能逐值比对（「undo 一整 turn 后所有 primitive 逐值
    /// 回退」是 M2 验收的核心句），顺序不定的快照比不出来。
    ///
    /// derived 一个都不在里面，也进不来——它们的键是 `DerivedKey`，另一张表
    /// （`graph::slot` 的裁决）。
    pub fn primitives(&self) -> Vec<(AtomKey, AgentValue)> {
        let ids: Vec<(AtomKey, agent_store::AtomId)> = self
            .sources
            .borrow()
            .iter()
            .map(|(key, id)| (key.clone(), id))
            .collect();
        let mut out: Vec<(AtomKey, AgentValue)> = ids
            .into_iter()
            .map(|(key, id)| (key, self.store.get(id)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 只读的 command log。011 的持久化从这里读整份日志，测试从这里数条目。
    ///
    /// 是 `&` 不是 `&mut`：日志的写入口只有命令（`step` / `begin_turn` / …），
    /// 借出可变引用等于给「手写一条 entry」开了门。
    pub fn history(&self) -> &AgentHistory {
        &self.history
    }

    /// 日志条数（含被 undo 掉、还能 redo 回来的尾巴）。
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// 游标 = 已生效条数。`history_len() - cursor()` 就是能 redo 回来的条数。
    pub fn cursor(&self) -> usize {
        self.history.cursor()
    }

    /// 最后一条 entry（物理最后，不一定是 undo 要弹的那一条）。测试与审计用。
    pub fn last_entry(&self) -> Option<&AgentEntry> {
        self.history.last()
    }

    /// 诊断探针：derived 到目前为止真的重算了多少次。
    ///
    /// 存在的理由是「undo 之后 derived **重算**一致」和「停在旧值碰巧也一致」在
    /// 断言上长得一模一样，只有这个计数分得开。跟 `agent-store` 的
    /// `debug_recompute_count` 一样是 `#[doc(hidden)]`——它不是公开面的一部分。
    #[doc(hidden)]
    pub fn debug_recompute_count(&self) -> usize {
        self.store.debug_recompute_count()
    }
}
