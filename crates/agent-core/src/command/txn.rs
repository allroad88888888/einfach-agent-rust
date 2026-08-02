//! 一次转移的写入事务：**红线 2 的物理收口**。
//!
//! 一个 [`Txn`] = 一次 `store.batch` = 一个 undo 步。转移表拿到的是它，不是 store
//! ——所以「每次 primitive 写入必有 `Entry`」不是纪律而是结构：写入的唯一入口
//! [`Txn::set`] 就是 `record_set`（捕获 `prev` → 写 → 产出一条 `Change`），
//! 而 `Txn` 手上根本没有别的写法。
//!
//! ## 为什么整个转移包在一个 batch 里
//!
//! 一次转移是一次状态跃迁，中间态不该被任何 derived 看见。不批就是每写一个
//! primitive 冲一次：`tools_converged` 会在「槽位已经更新、状态还没更新」的世界上
//! 重算若干次（glitch）。批到最后一次 flush，下游只在全部值就位之后重算。
//!
//! ## 类型化读写
//!
//! 槽位与 `AgentValue` 变体的对应关系由 `graph::Slot::default_value()` 一处焊死，
//! 这里的读取器全部 `expect`：对不上说明构图函数和读取点已经不同步，那是必须当场
//! 炸掉的 bug（本仓最恨的静默错值就长这样）。

use std::sync::Arc;

use agent_store::{AtomId, record_set};
use imbl::Vector;

use crate::engine::epoch::Epoch;
use crate::engine::state::{ToolSlot, TurnStatus};
use crate::graph::{
    AgentStore, AtomKey, DerivedFamily, DerivedKey, Slot, SourceFamily, derived_atom, source_atom,
};
use crate::ids::{AgentId, MessageId, ToolCallId};
use crate::seam::PrefixImage;
use crate::value::atom_value::AgentValue;
use crate::value::message::{ContentBlock, Message, Role};

use super::meta::AgentChange;

/// 一次转移写完之后交还给调用方的账：变更、屏障位、要不要 bump epoch。
///
/// 三样东西一起返回而不是让转移表各自去改 `Session`：转移表拿不到 `Session`
/// （它只看得见 [`Txn`]），于是「一次转移能对会话做什么」在类型上就是这三件，
/// 不会有第四件从某个角落里长出来。
pub(crate) struct Commit {
    pub(crate) changes: Vec<AgentChange>,
    pub(crate) barrier: bool,
    pub(crate) bump_epoch: bool,
}

/// 一次转移的写入事务。转移表只看得见这个类型。
pub(crate) struct Txn {
    store: AgentStore,
    sources: SourceFamily,
    derived: DerivedFamily,
    agent: AgentId,
    epoch: Epoch,
    irreversible: Vec<ToolCallId>,
    changes: Vec<AgentChange>,
    barrier: bool,
    bump_epoch: bool,
}

impl Txn {
    pub(crate) fn new(
        store: &AgentStore,
        sources: &SourceFamily,
        derived: &DerivedFamily,
        agent: &AgentId,
        epoch: Epoch,
        irreversible: &[ToolCallId],
    ) -> Self {
        Txn {
            store: store.clone(),
            sources: sources.clone(),
            derived: derived.clone(),
            agent: agent.clone(),
            epoch,
            irreversible: irreversible.to_vec(),
            changes: Vec::new(),
            barrier: false,
            bump_epoch: false,
        }
    }

    /// 收账。**调用方拿到 `changes` 之后必须把它交给 `History::append`**——落在地上
    /// 就是一次「写进了 store 却没进日志」的写入，正是红线 2 要挡的洞。
    pub(crate) fn finish(self) -> Commit {
        Commit {
            changes: self.changes,
            barrier: self.barrier,
            bump_epoch: self.bump_epoch,
        }
    }

    pub(crate) fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 当前世代。发 effect 时带上它（红线 6），结果回写前由 `Session::step` 的闸比对。
    pub(crate) fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// 取消要求 bump 世代——**但 epoch 不是 atom**，所以这里只是记一笔账，
    /// 真正的推进由 `Session` 在这一批落完之后做。理由见 `EntryMeta::epoch`：
    /// 世代只增不减，undo 不该把它回滚回去，而进了原子图的东西一定会被 undo 回滚。
    pub(crate) fn request_epoch_bump(&mut self) {
        self.bump_epoch = true;
    }

    /// 这一步记录的是一次不可逆操作的结果 → `EntryMeta.barrier = true`。
    pub(crate) fn mark_barrier(&mut self) {
        self.barrier = true;
    }

    /// 这次调用是宿主标记过的不可逆工具吗（见 `Session::mark_irreversible`）。
    pub(crate) fn is_irreversible(&self, call_id: &ToolCallId) -> bool {
        self.irreversible.contains(call_id)
    }

    fn atom(&self, slot: Slot) -> AtomId {
        source_atom(
            &self.store,
            &self.sources,
            &AtomKey::Agent(self.agent.clone(), slot),
        )
    }

    fn get(&self, slot: Slot) -> AgentValue {
        self.store.get(self.atom(slot))
    }

    /// **唯一的写入通道**。值没变就不落 `Change`（`record_set` 的 `PartialEq` 判定），
    /// 于是「一次转移什么都没改」自然不产生 entry，undo 里不会有幽灵步。
    fn set(&mut self, slot: Slot, next: AgentValue) {
        self.set_key(AtomKey::Agent(self.agent.clone(), slot), next);
    }

    /// 按**任意逻辑键**写——[`set`](Txn::set) 只是它填上本 agent 的那一版。
    ///
    /// 存在的唯一理由是 spawn / despawn：那两条命令天生跨 agent（父的一条命令要
    /// 记下子的初始槽位、子树的 teardown 值），而「整棵树共用一个 store、共用一条
    /// 日志」的全部好处正是从这里兑现的——子 agent 的诞生和消失跟父 agent 的那一步
    /// 落在**同一条 `Entry`** 上，于是 `undo_turn` 一次退回一整轮时它们同进同退，
    /// 不需要任何分布式事务。
    ///
    /// **转移表拿不到它**：它是 `pub(crate)`，而转移表只看得见 `Txn` 的那批类型化
    /// 读写口。一次普通转移写到别的 agent 头上是没有意义的（每个 agent 的轮状态
    /// 独立），能写就一定有人写。
    pub(crate) fn set_key(&mut self, key: AtomKey, next: AgentValue) {
        let atom = source_atom(&self.store, &self.sources, &key);
        self.changes.extend(record_set(&self.store, key, atom, next));
    }

    // —— 类型化读写 ————————————————————————————————————————

    pub(crate) fn status(&self) -> TurnStatus {
        self.get(Slot::Status)
            .as_status()
            .expect("Status 槽位持 Status")
            .clone()
    }

    pub(crate) fn set_status(&mut self, status: TurnStatus) {
        self.set(Slot::Status, AgentValue::Status(status));
    }

    pub(crate) fn messages(&self) -> Vector<Message> {
        self.get(Slot::Messages)
            .as_messages()
            .expect("Messages 槽位持 Messages")
            .clone()
    }

    pub(crate) fn tool_slots(&self) -> Arc<Vec<ToolSlot>> {
        self.get(Slot::ToolSlots)
            .as_slots()
            .expect("ToolSlots 槽位持 Slots")
            .clone()
    }

    pub(crate) fn set_tool_slots(&mut self, slots: Vec<ToolSlot>) {
        self.set(Slot::ToolSlots, AgentValue::Slots(Arc::new(slots)));
    }

    pub(crate) fn set_prev_prefix(&mut self, prefix: PrefixImage) {
        self.set(Slot::PrevPrefix, AgentValue::Prefix(prefix));
    }

    /// 清掉前缀镜像（027：CLI 的 `/model <name>` 切 provider 之后调）——跨家的
    /// 前缀比对没有意义，不清的话第 1 层会拿新家这次请求的裸字节去对旧家的
    /// 镜像，把正常的家族切换误报成漂移。
    pub(crate) fn clear_prev_prefix(&mut self) {
        self.set(Slot::PrevPrefix, AgentValue::Null);
    }

    fn count(&self, slot: Slot) -> u32 {
        self.get(slot).as_u64().expect("计数槽位持 U64") as u32
    }

    fn set_count(&mut self, slot: Slot, n: u32) {
        self.set(slot, AgentValue::U64(n as u64));
    }

    /// 铸一个新的 `MessageId` 并推进计数器（从 1 起严格递增）。
    fn mint_message_id(&mut self) -> MessageId {
        let n = self
            .get(Slot::NextMessageId)
            .as_u64()
            .expect("NextMessageId 槽位持 U64");
        self.set(Slot::NextMessageId, AgentValue::U64(n + 1));
        MessageId(n)
    }

    /// 铸号 + 造 `Message` + 追加进历史，一步到位并返回铸出的号。转移表造的每一条
    /// 消息都经这里——分成两步会给「铸了号但没追加」这种半成品状态留出空间。
    pub(crate) fn push_message(&mut self, role: Role, blocks: Vec<ContentBlock>) -> MessageId {
        let id = self.mint_message_id();
        let mut messages = self.messages();
        messages.push_back(Message { id, role, blocks });
        self.set(Slot::Messages, AgentValue::Messages(messages));
        id
    }

    /// 想发一次 `CallProvider`（新一轮或重试）之前先问它「预算还有吗」：到了
    /// `max_turns` 返回 `false`（不增），没到就 `turns_used += 1` 并返回 `true`。
    pub(crate) fn record_turn_attempt(&mut self) -> bool {
        let used = self.count(Slot::TurnsUsed);
        if used >= self.count(Slot::MaxTurns) {
            return false;
        }
        self.set_count(Slot::TurnsUsed, used + 1);
        true
    }

    /// 同上，管的是**这一条失败-重试链**的预算。
    pub(crate) fn record_retry_attempt(&mut self) -> bool {
        let used = self.count(Slot::RetriesUsed);
        if used >= self.count(Slot::MaxRetries) {
            return false;
        }
        self.set_count(Slot::RetriesUsed, used + 1);
        true
    }

    pub(crate) fn retries_used(&self) -> u32 {
        self.count(Slot::RetriesUsed)
    }

    pub(crate) fn max_retries(&self) -> u32 {
        self.count(Slot::MaxRetries)
    }

    pub(crate) fn clear_retries(&mut self) {
        self.set_count(Slot::RetriesUsed, 0);
    }

    /// 开新一轮时把本轮预算清零。两个计数一起清是刻意的：它们记的都是「**这一轮**
    /// 用掉了多少」，分开清会让「新一轮开局却带着上一轮的重试计数」成为可能。
    pub(crate) fn clear_turn_budget(&mut self) {
        self.set_count(Slot::TurnsUsed, 0);
        self.set_count(Slot::RetriesUsed, 0);
    }

    pub(crate) fn set_max_turns(&mut self, max_turns: u32) {
        self.set_count(Slot::MaxTurns, max_turns);
    }

    pub(crate) fn set_max_retries(&mut self, max_retries: u32) {
        self.set_count(Slot::MaxRetries, max_retries);
    }

    /// 收敛判断：读那个 derived atom（003 预言的落点），不在这里现扫。
    ///
    /// 走 derived 而不是就地 `tool_slots().iter().any(..)` 不是绕远路：它让「收敛」
    /// 成为图上的一个值，undo 回滚了槽位之后它**自动**跟着回来（重算），而就地扫
    /// 出来的布尔只活在这一次调用里，下游谁想用都得自己再扫一遍。
    pub(crate) fn tools_converged(&self) -> bool {
        let id = derived_atom(
            &self.store,
            &self.sources,
            &self.derived,
            &DerivedKey::ToolsConverged(self.agent.clone()),
        );
        matches!(self.store.get(id), AgentValue::Bool(true))
    }
}
