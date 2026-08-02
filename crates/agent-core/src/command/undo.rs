//! undo / redo 命令：**红线 6 在这里结账**（017 推过来的账）。
//!
//! 三步，缺一不可，顺序不能换：
//!
//! 1. 挪游标，把该应用的条目取出来（`History::undo_turn` / `redo_turn`，017/018）
//! 2. **bump 世代**——在写回状态**之前**。在飞的 effect 回来时比对的是新世代，
//!    于是它们全部被 `Session::step` 的闸挡掉。反过来（先写回再 bump）会留出一个
//!    窗口：一条回执刚好在这中间到达，写进一个已经被回滚掉的世界
//! 3. 把 `prev`（undo）/ `next`（redo）写回 store（`apply_prev` / `apply_next`，019）
//!
//! 第 3 步的 `resolve` 是 `graph::source_atom`——**get-or-create**，于是「这个 atom
//! 早就被逐出了」在 undo/redo 路径上根本不是一种情况，重建走的是平时建 atom 的
//! 同一行代码（019 的结论）。
//!
//! ## redo 不 bump 世代
//!
//! undo 是**放弃一个世界**：在飞的东西属于那个世界，必须作废。redo 是把状态追回到
//! 一个曾经存在过的点上，没有任何新的东西被放弃——那一代的在飞 effect 早在 undo
//! 那一下就已经作废了，再 bump 一次只是让世代号跑得更快，挡不住任何多余的东西。
//!
//! ## 两层粒度
//!
//! 决策 5 定的两档都在：`*_turn` 是 UI 默认档（027 的 `/undo` 用它），`*_step` 是
//! 「一条 entry」的开发者档（可展开的时间线）。两档共用同一条应用路径，差别只在问
//! `History` 要哪一批条目。

use std::cell::Cell;
use std::collections::BTreeSet;

use agent_store::{UndoOutcome, apply_next, apply_prev};

use crate::graph::{AtomKey, build_agent, source_atom};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

use super::meta::{AgentEntry, EntryMeta, is_barrier, same_turn};
use super::session::Session;

/// 一次 undo / redo 的结果。给 027 的 CLI 打印用（「回退了哪一轮、多少条目」）。
#[derive(Clone, PartialEq, Eq, Debug)]
#[must_use = "丢弃 UndoReport = 用户按了 undo 却不知道发生了什么（包括『被屏障挡住了』）"]
pub enum UndoReport {
    /// 走完了：`entries` 条属于 `turn_id` 这一轮的条目被回滚 / 重放。
    Applied { entries: usize, turn_id: u64 },
    /// 撞上屏障（`EntryMeta.barrier`，即 020 的 `Irreversible` 工具）。
    ///
    /// `entries` 是**已经**回滚掉的条数（比屏障新的那些——它们的 `prev` 链在屏障
    /// 之上，自洽；退回去问用户期间把它们留在新值上，等于状态处于一个日志里不存在
    /// 的中间态）。`barrier_seq` 那一条停在门口**没被越过**。
    ///
    /// 用户确认「继续，副作用不回滚」= 再调一次 [`Session::undo_turn_force`]。
    Blocked { entries: usize, barrier_seq: u64 },
    /// 无可做（游标已在端点）。
    Nothing,
}

impl Session {
    /// 回退一整个 turn（决策 5 的默认档）：从游标处连续弹掉 `turn_id` 相同的条目，
    /// 跨过 turn 边界即停。
    ///
    /// 撞上 `barrier=true` 的条目 → [`UndoReport::Blocked`]，游标停在屏障后一格。
    /// `History` **不记「这条已经问过了」**：越过永远是上层的一次显式决定
    /// （[`undo_turn_force`](Session::undo_turn_force)），不会因为某个状态位而在
    /// 下一次 undo 里被静默继承。
    ///
    /// 子 agent 的 entry 继承所在 root turn 的 `turn_id`、不产生新边界，所以一次
    /// `undo_turn` 会连带退掉那一轮里所有子 agent 的工作——这正是「整棵树共用一个
    /// store」应有的语义。
    pub fn undo_turn(&mut self) -> UndoReport {
        let outcome = self.history.undo_turn(same_turn, is_barrier);
        self.rewind(outcome)
    }

    /// 回退**一条** entry（决策 5 的开发者档 / 可展开时间线）。屏障判定与
    /// [`undo_turn`](Session::undo_turn) 一致。
    ///
    /// 一条 entry = 一次 `store.batch` = 一次转移。于是「退回工具结果落地之前」
    /// 这种 turn 内部的位置只有这一档到得了。
    pub fn undo_step(&mut self) -> UndoReport {
        let outcome = self.history.undo_one(is_barrier);
        self.rewind(outcome)
    }

    /// [`undo_step`](Session::undo_step) 的反演。
    pub fn redo_step(&mut self) -> UndoReport {
        let outcome = self.history.redo_one();
        self.fast_forward(outcome)
    }

    /// 越过**第一条**屏障再退（027 的 `/undo!` 后端）。
    ///
    /// 「第一条」不是「全部」：一次确认只放行一条不可逆操作。用户看到的提示说的是
    /// 「越过的是这一个 `shell/exec`」，那就只该越过这一个；同一轮里还有第二个不可逆
    /// 操作时再停一次、再问一次。放行全部等于让一次确认替用户答了几个他没被问到的
    /// 问题。
    pub fn undo_turn_force(&mut self) -> UndoReport {
        let crossed = Cell::new(false);
        let outcome = self.history.undo_turn(same_turn, move |meta| {
            if !meta.barrier {
                return false;
            }
            if crossed.get() {
                return true;
            }
            crossed.set(true);
            false
        });
        self.rewind(outcome)
    }

    /// 反演一次 [`undo_turn`](Session::undo_turn)：把 `next` 一路写回去。
    ///
    /// **没有屏障参数**：redo 只是把值写回状态，不重放外部副作用。undo 挡的是
    /// 「状态回滚到副作用发生之前、和已经发生的外部世界对不上」，redo 是把状态追回到
    /// 外部世界已经在的那个点上——方向反了，理由不成立。
    pub fn redo_turn(&mut self) -> UndoReport {
        let outcome = self.history.redo_turn(same_turn);
        self.fast_forward(outcome)
    }

    /// undo 侧的应用路径：bump 世代 → `apply_prev`。
    fn rewind(&mut self, outcome: Outcome) -> UndoReport {
        let (entries, report) = match outcome {
            UndoOutcome::Nothing => return UndoReport::Nothing,
            UndoOutcome::Applied(entries) => {
                let report = UndoReport::Applied {
                    entries: entries.len(),
                    turn_id: turn_id_of(&entries),
                };
                (entries, report)
            }
            UndoOutcome::Blocked { applied, barrier_seq } => {
                let report = UndoReport::Blocked { entries: applied.len(), barrier_seq };
                (applied, report)
            }
        };
        if entries.is_empty() {
            // 屏障就在游标下：游标一动不动，一个字节都没改——不该 bump 世代
            // （bump 会白白作废一批还有效的在飞 effect），也不该走 applier。
            return report;
        }

        // 红线 6：**先 bump 再写回**。见模块文档。
        self.epoch = self.epoch.next();
        self.rebuild_touched_agents(&entries);
        let (store, sources) = (self.store.clone(), self.sources.clone());
        let mut resolve = |key: &AtomKey| source_atom(&store, &sources, key);
        apply_prev(&self.store, &mut resolve, &entries);
        report
    }

    /// redo 侧的应用路径：`apply_next`，**不 bump 世代**（见模块文档）。
    fn fast_forward(&mut self, outcome: Outcome) -> UndoReport {
        let UndoOutcome::Applied(entries) = outcome else {
            // redo 只可能返回 `Applied` 或 `Nothing`——它压根没有屏障谓词。
            return UndoReport::Nothing;
        };
        let report = UndoReport::Applied {
            entries: entries.len(),
            turn_id: turn_id_of(&entries),
        };
        self.rebuild_touched_agents(&entries);
        let (store, sources) = (self.store.clone(), self.sources.clone());
        let mut resolve = |key: &AtomKey| source_atom(&store, &sources, key);
        apply_next(&self.store, &mut resolve, &entries);
        report
    }

    /// 这一批条目碰到的每个 agent，先把它的**整张图**补齐（`build_agent` 是
    /// get-or-create，已在的原样返回）。
    ///
    /// # 为什么不能只靠 applier 的 `resolve`
    ///
    /// `resolve` 是按**键**的 get-or-create：它只建条目里出现过的那几个 atom。
    /// 一个被 despawn 逐出的子 agent，undo 回来时条目里带的是它被 teardown 的那些
    /// 槽位——数量上正好是全部（teardown 写了每一个），但**它的 derived 不在日志里**
    /// （derived 全部可重算，这正是「完整状态 = 所有 primitive」成立的原因）。
    /// 不在这里补，那个 agent 的 `ToolsConverged` 就要等到下一次有人读它才重建；
    /// 更糟的是 029 之后条目里只带部分槽位的情况——剩下的槽位不在 family 里，
    /// `Session::primitives()` 就少一项，快照跟着少一项，恢复时那一项落默认值
    /// **而且永远不报错**。
    ///
    /// # 为什么放在 applier 之外
    ///
    /// 019 推过来的账：**`resolve` 闭包里不要读 store**。`build_agent` 末尾要读一次
    /// derived 把反向边装上，而那个 derived 会现查 family——如果这一下发生在
    /// applier 的 `resolve` 里（applier 正持着 batch），就是一次可以避免的重入。
    /// 摆在前面，代价只是多一次 get-or-create 遍历。
    fn rebuild_touched_agents(&self, entries: &[AgentEntry]) {
        let agents: BTreeSet<AgentId> = entries
            .iter()
            .flat_map(|entry| entry.changes.iter())
            .map(|change| change.key.agent().clone())
            .collect();
        for agent in &agents {
            build_agent(&self.store, &self.sources, &self.derived, agent);
        }
    }
}

/// `History` 交还的产物。取个别名是因为三个泛型参数在签名里念一遍就占两行。
type Outcome = UndoOutcome<AtomKey, AgentValue, EntryMeta>;

/// `Applied` 里的条目全部属于同一个 turn（`same_turn` 就是这么判的），取第一条的。
fn turn_id_of(entries: &[AgentEntry]) -> u64 {
    entries
        .first()
        .map(|entry| entry.meta.turn_id)
        .expect("Applied / 非空 Blocked 至少有一条")
}
