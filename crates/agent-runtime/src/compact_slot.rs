//! 摘要子 agent 的等待登记：记的是「哪个子 agent 在替哪个父 agent 的哪次
//! `Effect::Compact` 干活」，以及它到终态之后该回给父哪个事件（106）。
//!
//! # 为什么不能复用 [`crate::child_slot::ChildSlots`]
//!
//! 那张表收敛的是父的**tool-call 槽**：登记时要一个 `ToolCallId`，落终态时回写
//! `Event::ToolResult` / `Event::ToolFailed`。`Effect::Compact` 根本不是一次工具
//! 调用——core 那边没有 `call_id`（`engine/effect.rs` 的 `Compact` 变体只有
//! `agent` / `upto` / `epoch`），落终态时要回写的是 105 定的另两个事件
//! `Event::CompactDone` / `Event::CompactFailed`。硬塞进 `ChildSlots` 只会让那个
//! 类型多出一个「这一格其实没有 call_id」的特殊分支，两条记账不该合用一张表。
//!
//! # 结果翻译复用 [`crate::child_outcome::outcome`]
//!
//! 子 agent 的终态 → 文本 + 是否失败，这件事跟前台 spawn 收割是同一个问题（都是
//! 「子的最后一句话给父听」），翻译规则也该一样：`Done` 给它的终答，
//! `Done{truncated:true}` 给带说明的半份答案，`Failed` 一律算失败。这里唯一不同的
//! 是失败/成功之后事件的**形状**（`CompactDone`/`CompactFailed` 而非
//! `ToolResult`/`ToolFailed`），不是翻译规则本身。
//!
//! # `upto` 只住在这里（107 → 108 的硬契约）
//!
//! 回执事件里没有 `upto`（105 定死的形状），`Session` 里也还没有（正要写进去的
//! 就是它）。所以这张表除了「谁在替谁干活」之外还记着「这次盖到哪」，收割时把它
//! 转存成一份 [`PendingSummary`](crate::compact_writeback::PendingSummary)，
//! 等 [`crate::compact_writeback::after_step`] 判过 epoch 闸再取走。
//!
//! # 收割完就 despawn（108 裁决）
//!
//! `AgentLimits.max_children` 默认 **8**。不回收的话长会话自动压 8 次之后，之后
//! 每一次压缩都会 `SpawnRefused::TooManyChildren` → `CompactFailed`——**自动压缩
//! 从此永久失效**。它不红任何红线（每次都响亮地报一条 `Notice::CompactionFailed`），
//! 但整条 M12 就是为长会话做的，而它规定了长会话最多只能压 8 次。
//!
//! 摘要子是**纯粹的一次性工人**：输出已经被复制进回执事件（随后进父的
//! `Slot::Summaries`，107），它自己的历史之后没有任何人需要；undo / redo 不受
//! 影响（摘要正文住在父那边）；109 要展示的原文走父的完整记录、摘要正文走
//! `Slot::Summaries`，都不经过这个子 agent。despawn 的语义与 undo 行为 028/029
//! 已经定死并有测试，这里只是去调用它。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Event, Session};

use crate::child_outcome;
use crate::compact_writeback::PendingSummary;

struct CompactSlot {
    child: AgentId,
    /// 这次压缩归属的 agent——回执要喂给它（`Event::CompactDone`/`CompactFailed`
    /// 的 `agent` 字段），不是喂给 `child`。
    agent: AgentId,
    /// spawn 摘要子 agent 那一刻的世代。原样带回事件里；红线 6 那道真正的闸在
    /// `Session::step` 入口，这里不重复判断，只负责别把它弄丢。
    epoch: Epoch,
    /// 这次摘要要盖住的边界。见模块文档「`upto` 只住在这里」。
    upto: usize,
}

/// 本轮所有在飞的摘要子 agent，以及它们已经收割、正在等着过闸的结果。跟
/// [`crate::subtree::Subtree`] 同款：`resume` 每次重建，turn 内生死，不跨
/// `run_turn`（子 agent 不跨 turn 是 ORCHESTRATION §二定的结构性约束，摘要子
/// agent 不是例外）。
#[derive(Default)]
pub(crate) struct CompactSlots {
    slots: Vec<CompactSlot>,
    /// 已经收割成 `Event::CompactDone`、还没过 epoch 闸的回写意图。
    awaiting_gate: Vec<PendingSummary>,
}

impl CompactSlots {
    /// 记一笔：`child` 在替 `agent` 的这次压缩（世代 `epoch`、盖到 `upto`）干活。
    pub(crate) fn record(&mut self, child: AgentId, agent: AgentId, epoch: Epoch, upto: usize) {
        self.slots.push(CompactSlot {
            child,
            agent,
            epoch,
            upto,
        });
    }

    /// 终态的摘要子 agent 按登记顺序收割成喂回泵的事件；未终止的原样留着。
    /// 成功那一支同时把回写意图记进 `awaiting_gate`，并**当场 despawn** 这个
    /// 一次性工人（见模块文档）。
    ///
    /// 需要 `&mut Session` 就是为了那一下 despawn。**不在这里 `persist::sync`**：
    /// 这个函数每划掉一格必产出一个事件，泵下一圈处理它时无条件调一次 sync
    /// （`runner` 的 A 段），中间没有任何 IO，teardown 那条 entry 不会滞留。
    pub(crate) fn harvest(&mut self, session: &mut Session) -> Vec<Event> {
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.slots.len() {
            let status = session.status_of(&self.slots[i].child);
            if !status.is_terminal() {
                i += 1;
                continue;
            }
            let slot = self.slots.remove(i);
            let (content, is_error) = child_outcome::outcome(session, &slot.child, &status);
            events.push(if is_error {
                Event::CompactFailed {
                    agent: slot.agent.clone(),
                    epoch: slot.epoch,
                }
            } else {
                let summary: Arc<str> = Arc::from(content);
                self.awaiting_gate.push(PendingSummary {
                    agent: slot.agent.clone(),
                    epoch: slot.epoch,
                    upto: slot.upto,
                    summary: Arc::clone(&summary),
                });
                Event::CompactDone {
                    agent: slot.agent.clone(),
                    summary,
                    epoch: slot.epoch,
                }
            });
            reap(session, &slot.child);
        }
        events
    }

    /// 过闸之后取走这一份回写意图（`agent` + 世代都得对上）。
    pub(crate) fn take_gated_summary(
        &mut self,
        agent: &AgentId,
        epoch: Epoch,
    ) -> Option<PendingSummary> {
        let at = self
            .awaiting_gate
            .iter()
            .position(|pending| &pending.agent == agent && pending.epoch == epoch)?;
        Some(self.awaiting_gate.remove(at))
    }

    /// 世代已经推走的回写意图当场丢掉——epoch 只增不减，对不上就永远对不上了。
    pub(crate) fn drop_stale_summaries(&mut self, now: Epoch) {
        self.awaiting_gate.retain(|pending| pending.epoch == now);
    }
}

/// 拆掉一个干完活的摘要子 agent。
///
/// 拒绝（`DespawnRefused`）在这里**结构上不可达**：摘要子没有子孙、没有任何跨
/// agent 的读者（它的结果是被复制进事件的，不是被谁读着的），也不是 root。真撞上
/// 了也只是少回收一格——它已经是终态，不会再烧任何 token，下一次 spawn 撞
/// `TooManyChildren` 时照旧响亮地报 `CompactionFailed`。为一条不可达的分支新造一
/// 个通报变体，会连锁改 `SessionEvent`（跨 SSE 的协议枚举）→ 生成的 TS →
/// fixtures（054 的教训），不值。
fn reap(session: &mut Session, child: &AgentId) {
    let _ = session.despawn_child(child);
}

#[cfg(test)]
#[path = "compact_slot_tests.rs"]
mod tests;
