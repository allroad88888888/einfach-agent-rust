//! 一次转移 → 一个 batch → 一条 `Entry`。**这是「写入必须收口」的最后一米。**
//!
//! `docs/STATE-MODEL.md`：「`store.batch(|s| {…})` 一次 = 一个 undo 步。事务边界直接
//! 复用 batch，不另造概念。」这个文件就是那句话的实现，一共做四件事：
//!
//! 1. 开 batch，把 [`Txn`] 交给转移表
//! 2. 收账（[`Commit`](super::txn::Commit)：变更 / 可撤销档位 / 要不要 bump epoch）
//! 3. 该 bump 就 bump 世代（取消走这条；undo 那条在 [`undo`](super::undo)）
//! 4. 把这一批变更连同 [`EntryMeta`] 追加进日志
//!
//! 第 4 步**不判断变更是不是空的**：`History::append` 自己拒绝空步（009 的
//! 「幽灵步不落条目」），于是「协议违规不留 undo 痕迹」不需要在这里再判一次
//! ——判两次就有两处可以判错。

use crate::ids::AgentId;

use super::meta::EntryMeta;
use super::session::Session;
use super::txn::Txn;

impl Session {
    /// 跑一条 **root agent 的**命令。会话级命令（`begin_turn` / `set_max_*` /
    /// `clear_prev_prefix`）走它。
    pub(super) fn commit<R>(&mut self, label: &'static str, f: impl FnOnce(&mut Txn) -> R) -> R {
        let root = self.agent.clone();
        self.commit_as(&root, label, f)
    }

    /// 跑一条命令：`f` 里的每一次写入经 `Txn` → `record_set` → 一条 `Change`，
    /// 整批落成一条 `Entry`。返回 `f` 的产物（转移表返回的是 `Vec<Effect>`）。
    ///
    /// `agent` 是这一步**替谁做的**：`Session::step` 从事件里取（028 起事件的
    /// `agent` 字段真正路由），spawn / despawn 取发起那一方。它决定 `Txn` 的槽位
    /// 落在谁头上——每个 agent 的轮状态（status / 工具槽 / 预算）因此天然独立，
    /// 不需要「per-agent 的一份 TurnState」这种第二真值源。
    ///
    /// `EntryMeta.turn_id` 用的是**会话的**（root 铸的）那个号，不是 per-agent 的
    /// ——决策 5：子 agent 的 entry 继承所在 root turn 的 turn_id、不产生新的 turn
    /// 边界，于是 `undo_turn` 一次退回一整个 root turn，连带那一轮里所有子 agent
    /// 的工作。
    ///
    /// `EntryMeta.epoch` 记的是**写入时**的世代，不是 bump 之后的：这一步发生在
    /// 那个世界里。取消这一步因此记的是被取消的那一代——审计时「这一步属于哪一代」
    /// 和「它把世代推到了几」是两个问题，前者才是日志该回答的。
    pub(super) fn commit_as<R>(
        &mut self,
        agent: &AgentId,
        label: &'static str,
        f: impl FnOnce(&mut Txn) -> R,
    ) -> R {
        let epoch_at_write = self.epoch;
        let is_root = agent == &self.agent;
        let mut txn = Txn::new(
            &self.store,
            &self.sources,
            &self.derived,
            agent,
            is_root,
            epoch_at_write,
            &self.tool_marks,
        );

        // batch 的句柄是 store 的一份克隆（`Store` 是 `Rc` 句柄，克隆即共享）：
        // 闭包要可变借用 `txn` 和 `out`，而 `self.store.batch(..)` 会不可变借用
        // `self`，两者不能同时成立。
        let store = self.store.clone();
        let mut out = None;
        store.batch(|_| out = Some(f(&mut txn)));

        let commit = txn.finish();
        if commit.bump_epoch {
            self.epoch = self.epoch.next();
        }
        // 211：跟 `bump_epoch` 并排——两者都是**图外**的会话状态，由转移表提出
        // 请求、这里落地。自驱动预算不进原子图，`/undo` 因此天然退不还它
        // （钱已经烧掉了），而「所有 primitive 都跟着 undo 走」那句话一个字
        // 都不用改。
        if commit.refill_auto_turns {
            self.auto_turn_budget = self.limits.max_auto_turns;
        }
        let meta = EntryMeta {
            turn_id: self.turn_id,
            epoch: epoch_at_write,
            label,
            undoability: commit.undoability,
        };
        let _ = self.history.append(meta, commit.changes);

        out.expect("store.batch 同步执行闭包")
    }
}
