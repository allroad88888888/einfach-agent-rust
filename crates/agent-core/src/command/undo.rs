//! undo / redo 命令：**红线 6 在这里结账**（017 推过来的账）。
//!
//! 四步，缺一不可，顺序不能换：
//!
//! 1. 挪游标，把该应用的条目取出来（`History::undo_turn` / `redo_turn`，017/018）
//! 2. **bump 世代**——在跑钩子和写回状态**之前**。在飞的 effect 回来时比对的是新
//!    世代，于是它们全部被 `Session::step` 的闸挡掉。反过来（先跑再 bump）会留出
//!    一个窗口：一条回执刚好在这中间到达，写进一个已经被回滚掉的世界
//! 3. 逆序逐条跑**还原钩子**，`Ok` 了才回滚这一条（199 §三，见
//!    [`undo_hook`](super::undo_hook)）
//! 4. 把 `prev`（undo）/ `next`（redo）写回 store（`apply_prev` / `apply_next`，019）
//!
//! 第 4 步的 `resolve` 是 `graph::source_atom`——**get-or-create**，于是「这个 atom
//! 早就被逐出了」在 undo/redo 路径上根本不是一种情况，重建走的是平时建 atom 的
//! 同一行代码（019 的结论）。
//!
//! ## 第 3 步为什么在第 4 步之前
//!
//! 还原函数碰的是外部世界，`apply_prev` 碰的是 store。先回滚 store 再去还原，一旦
//! 还原失败就是「store 说没发生、CRM 说发生了」——红线导言点名的静默错值。整张账
//! 在 [`undo_hook`](super::undo_hook) 的模块文档里。
//!
//! ## redo 不 bump 世代、也不跑钩子
//!
//! undo 是**放弃一个世界**：在飞的东西属于那个世界，必须作废。redo 是把状态追回到
//! 一个曾经存在过的点上，没有任何新的东西被放弃——那一代的在飞 effect 早在 undo
//! 那一下就已经作废了，再 bump 一次只是让世代号跑得更快，挡不住任何多余的东西。
//! 钩子同理：redo 只是把值写回状态，**不重放外部副作用**，所以既不跑还原钩子、
//! 也没有什么正向钩子可跑（200 §5 明确不动）。
//!
//! ## 两层粒度 / 无参版本 = 递一个恒 `Ok` 的钩子
//!
//! 决策 5 定的两档都在：`*_turn` 是 UI 默认档（027 的 `/undo` 用它），`*_step` 是
//! 「一条 entry」的开发者档（可展开的时间线）。两档共用同一条应用路径，差别只在问
//! `History` 要哪一批条目。
//!
//! [`undo_turn`](Session::undo_turn) / [`undo_step`](Session::undo_step) /
//! [`undo_turn_force`](Session::undo_turn_force) 保留原签名，等价于对应的 `*_with`
//! 传 [`always_ok`]（CLI 之外还有 wasm / server / 测试在调，一次全改是无谓的爆炸
//! 半径）。没有 `Hooked` 条目的会话里两者逐字节等价——钩子只对它发问。

use std::cell::Cell;
use std::collections::BTreeSet;

use agent_store::{UndoOutcome, apply_next};

use crate::graph::{AtomKey, build_agent, source_atom};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

use super::meta::{AgentEntry, EntryMeta, is_barrier, same_turn};
use super::session::Session;
use super::undo_hook::{BlockedCause, Crossing, HookOutcome};

/// 一次 undo / redo 的结果。给 027 的 CLI 打印用（「回退了哪一轮、多少条目」）。
#[derive(Clone, PartialEq, Eq, Debug)]
#[must_use = "丢弃 UndoReport = 用户按了 undo 却不知道发生了什么（包括『被屏障挡住了』）"]
pub enum UndoReport {
    /// 走完了：`entries` 条属于 `turn_id` 这一轮的条目被回滚 / 重放。
    Applied { entries: usize, turn_id: u64 },
    /// 停下来问用户。`cause` 说明**为什么**停（199 §五）：屏障是「没碰」，
    /// 钩子失败 / 钩子丢失是「碰了，而且可能做了一半」——三种话术不同，用户据此
    /// 决定要不要强制越过。
    ///
    /// `entries` 是**已经**回滚掉的条数（比停下那一条新的那些——它们的 `prev` 链在它
    /// 之上，自洽；留在新值上等于状态处于一个日志里不存在的中间态）。`barrier_seq`
    /// 那一条停在门口**没被越过**，它的状态不回滚。
    ///
    /// 用户确认「继续，副作用不回滚」= 再调一次 [`Session::undo_turn_force`]。
    Blocked {
        entries: usize,
        barrier_seq: u64,
        cause: BlockedCause,
    },
    /// 无可做（游标已在端点）。
    Nothing,
}

/// 「没有任何还原钩子」的钩子：无参 undo 口子递给 `*_with` 的就是它。
///
/// 恒 `Ok` 而不是恒 `Lost`：「这一版调用方压根没有钩子表」和「有表但这一条查不到」
/// 是两回事，后者才是崩溃恢复之后那种「说好有函数、函数没了」。
pub fn always_ok(_: &AgentEntry) -> HookOutcome {
    HookOutcome::Ok
}

impl Session {
    /// 回退一整个 turn（决策 5 的默认档）：从游标处连续弹掉 `turn_id` 相同的条目，
    /// 跨过 turn 边界即停。
    ///
    /// 撞上 `Undoability::Blocked` 的条目 → [`UndoReport::Blocked`]，游标停在屏障
    /// 后一格。`History` **不记「这条已经问过了」**：越过永远是上层的一次显式决定
    /// （[`undo_turn_force`](Session::undo_turn_force)），不会因为某个状态位而在
    /// 下一次 undo 里被静默继承。
    ///
    /// 子 agent 的 entry 继承所在 root turn 的 `turn_id`、不产生新边界，所以一次
    /// `undo_turn` 会连带退掉那一轮里所有子 agent 的工作——这正是「整棵树共用一个
    /// store」应有的语义。
    pub fn undo_turn(&mut self) -> UndoReport {
        self.undo_turn_with(&mut always_ok)
    }

    /// [`undo_turn`](Session::undo_turn) + 还原钩子（199 §三）。
    ///
    /// `run_hook` 只会被 `Undoability::Hooked` 的条目问到，逆序、一条一条问，问完
    /// 一条 `Ok` 就立刻回滚那一条的状态。**core 不认识 `UndoFn`**：这里收的是调用方
    /// 递进来的回调，函数本体住 runtime 的钩子表（红线 7，199 §二）。
    pub fn undo_turn_with(
        &mut self,
        run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome,
    ) -> UndoReport {
        let outcome = self.history.undo_turn(same_turn, is_barrier);
        self.rewind(outcome, run_hook, Crossing::Stop)
    }

    /// 回退**一条** entry（决策 5 的开发者档 / 可展开时间线）。屏障判定与
    /// [`undo_turn`](Session::undo_turn) 一致。
    ///
    /// 一条 entry = 一次 `store.batch` = 一次转移。于是「退回工具结果落地之前」
    /// 这种 turn 内部的位置只有这一档到得了。
    pub fn undo_step(&mut self) -> UndoReport {
        self.undo_step_with(&mut always_ok)
    }

    /// [`undo_step`](Session::undo_step) + 还原钩子，语义同
    /// [`undo_turn_with`](Session::undo_turn_with)。
    pub fn undo_step_with(
        &mut self,
        run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome,
    ) -> UndoReport {
        let outcome = self.history.undo_one(is_barrier);
        self.rewind(outcome, run_hook, Crossing::Stop)
    }

    /// [`undo_step`](Session::undo_step) 的反演。
    pub fn redo_step(&mut self) -> UndoReport {
        let outcome = self.history.redo_one();
        self.fast_forward(outcome)
    }

    /// 越过**第一条**障碍再退（027 的 `/undo!` 后端）。
    ///
    /// 「第一条」不是「全部」：一次确认只放行一个障碍。用户看到的提示说的是
    /// 「越过的是这一个 `shell/exec`」，那就只该越过这一个；同一轮里还有第二个不可逆
    /// 操作（或第二条还原失败）时再停一次、再问一次。放行全部等于让一次确认替用户
    /// 答了几个他没被问到的问题。
    ///
    /// 「障碍」现在有两种：屏障（`Undoability::Blocked`，**没碰**）和还原失败（钩子
    /// 跑挂了 / 随进程重启没了，**碰了**）。两者共用这条放行路径、也共用「一次一条」
    /// 的额度——199 §五 的原话：还原失败复用屏障，不新开机制。
    pub fn undo_turn_force(&mut self) -> UndoReport {
        self.undo_turn_force_with(&mut always_ok)
    }

    /// [`undo_turn_force`](Session::undo_turn_force) + 还原钩子。
    ///
    /// 这里的谓词只放行 `Blocked`（`History` 那一侧只看得见 `&EntryMeta`，判不出
    /// 一条钩子跑不跑得成）。**放行额度真正花在哪一个障碍上，由 `Crossing::CrossOne`
    /// 那一层的逐条循环决定**——理由见 `undo_hook.rs` 里那个类型的文档。
    pub fn undo_turn_force_with(
        &mut self,
        run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome,
    ) -> UndoReport {
        let crossed = Cell::new(false);
        let outcome = self.history.undo_turn(same_turn, move |meta| {
            if !is_barrier(meta) {
                return false;
            }
            if crossed.get() {
                return true;
            }
            crossed.set(true);
            false
        });
        self.rewind(outcome, run_hook, Crossing::CrossOne)
    }

    /// 反演一次 [`undo_turn`](Session::undo_turn)：把 `next` 一路写回去。
    ///
    /// **没有屏障参数、也没有钩子参数**：redo 只是把值写回状态，不重放外部副作用。
    /// undo 挡的是「状态回滚到副作用发生之前、和已经发生的外部世界对不上」，redo 是
    /// 把状态追回到外部世界已经在的那个点上——方向反了，理由不成立。
    pub fn redo_turn(&mut self) -> UndoReport {
        let outcome = self.history.redo_turn(same_turn);
        self.fast_forward(outcome)
    }

    /// undo 侧的应用路径：bump 世代 → 逐条（钩子 → `apply_prev`）。
    fn rewind(
        &mut self,
        outcome: Outcome,
        run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome,
        crossing: Crossing,
    ) -> UndoReport {
        let (entries, barrier_seq) = match outcome {
            UndoOutcome::Nothing => return UndoReport::Nothing,
            UndoOutcome::Applied(entries) => (entries, None),
            UndoOutcome::Blocked {
                applied,
                barrier_seq,
            } => (applied, Some(barrier_seq)),
        };
        if entries.is_empty() {
            // 屏障就在游标下：游标一动不动，一个字节都没改——不该 bump 世代
            // （bump 会白白作废一批还有效的在飞 effect），也不该走 applier。
            return UndoReport::Blocked {
                entries: 0,
                barrier_seq: barrier_seq.expect("空的 Applied 不存在：undo_while 至少弹一条"),
                cause: BlockedCause::NoHook,
            };
        }
        let turn_id = turn_id_of(&entries);

        // 红线 6：**先 bump 再跑钩子再写回**。见模块文档。钩子失败停在第一条时
        // 这一下等于白 bump 了一代——那是安全的方向（多作废一批在飞 effect），
        // 反过来才是红线要挡的窗口。
        self.epoch = self.epoch.next();

        let Some(stop) = self.unwind(&entries, run_hook, crossing) else {
            return match barrier_seq {
                Some(barrier_seq) => UndoReport::Blocked {
                    entries: entries.len(),
                    barrier_seq,
                    cause: BlockedCause::NoHook,
                },
                None => UndoReport::Applied {
                    entries: entries.len(),
                    turn_id,
                },
            };
        };
        // 没退掉的那些条目游标已经替它们记过账了，推回去（见 `recede_cursor`）。
        self.recede_cursor(entries.len() - stop.undone);
        UndoReport::Blocked {
            entries: stop.undone,
            barrier_seq: stop.seq,
            cause: stop.cause,
        }
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
    /// # undo 侧为什么按**条**补而不是整批一次补
    ///
    /// 199 之后 undo 可能停在半路（还原钩子失败）。整批一次补 = 给一批**不会被
    /// 回滚**的条目也建了图：其中若有一条 despawn，那个子 agent 会带着一整套默认值
    /// 复活进 family，`primitives()` 里凭空多出一个不该存在的 agent，快照跟着多，
    /// 而它从头到尾不报错。按条补，「补了图」和「退了状态」永远同进同退。
    pub(super) fn rebuild_touched_agents(&self, entries: &[AgentEntry]) {
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
