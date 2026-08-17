//! undo 路的**逐条循环**：先跑还原钩子，`Ok` 了才回滚这一条的状态（决策 199 §三）。
//!
//! 从 [`undo`](super::undo) 拆出来的理由是红线 9（那个文件留 `undo_*`/`redo_*` 的
//! 公开口就已经顶到 300 行），但拆开之后两件事也确实分得开：那边回答「这一次 undo
//! 要动哪一批条目」，这边回答「这一批里每一条能不能真的退掉」。
//!
//! ## 顺序：为什么钩子必须跑在 `apply_prev` 之前
//!
//! | 顺序 | 还原失败时 | 判定 |
//! |---|---|---|
//! | 先跑还原函数，成功了再回滚 store | store 没动，外部没动，**一致** | ✅ |
//! | 先回滚 store，再跑还原函数 | store 说没发生，CRM 说发生了 | ❌ **红线导言点名的静默错值** |
//!
//! 写反了不报错、不 panic，只在「还原函数失败」这条罕见路径上以状态自相矛盾的
//! 形式浮出来。`undo_hook_tests.rs` 的第一条测试专门钉它：钩子在失败的那一刻读
//! 一次 store，读到的**必须**是回滚前的值。
//!
//! ## 逆序，而且不许「顺手优化成并行」
//!
//! 论文 Theorem 16 证了按逆序（LIFO）撤销**不需要任何前提**；任意顺序要求 effects
//! 两两独立（Corollary 21），而那是我们无法验证的性质。journal 本来就是逆序走的，
//! 所以这条不需要额外工作——但它是**要求**不是巧合，改动这个循环时别把它优化掉。
//!
//! ## core 不认识 `UndoFn`（红线 7）
//!
//! 还原函数会碰外部世界，`agent-core` 不做 IO、不能持有那种闭包。这里收的是调用方
//! 递进来的一个 `&mut dyn FnMut(&AgentEntry) -> HookOutcome`——跟
//! `History::undo_turn(same_turn, is_barrier)` 今天已经收谓词参数是同一个形状，
//! 不是新发明。回调收 `&AgentEntry` 而不是 `&EntryMeta`：runtime 的钩子表按
//! `Entry::seq` 查，而 `seq` 在 `Entry` 上不在 `meta` 上（199 §九：能不加字段就不加）。

use std::sync::Arc;

use agent_store::apply_prev;

use crate::graph::{AtomKey, source_atom};

use super::meta::{AgentEntry, Undoability};
use super::session::Session;

/// 一条 entry 的还原钩子跑出来是什么结果。
///
/// core 只在这一条 entry 是 [`Undoability::Hooked`] 时才问——别的两档不需要问：
/// `StateOnly` 没碰外部世界，`Blocked` 压根进不到这个循环（`History` 的屏障谓词
/// 先把它挡在门外了）。
pub enum HookOutcome {
    /// 钩子跑成功了——可以回滚这一条的状态。
    Ok,
    /// 钩子跑了但**失败了**：碰了，而且可能做了一半。附一句给用户看的原因。
    Failed(Arc<str>),
    /// 这一条声明了 `Hooked`，但钩子表里查不到它——**函数随进程重启没了**。
    ///
    /// 单独一档而不是复用 [`HookOutcome::Failed`]：话术不同。「跑了没成」和
    /// 「重启之后没得跑」对用户是两件事，而两者都要他自己决定要不要强制越过。
    Lost,
}

/// [`UndoReport::Blocked`](super::undo::UndoReport::Blocked) 的成因（199 §五）。
///
/// 三种话术不同，这正是加成因的全部理由：屏障是「**没碰**」（不知道怎么撤，停在
/// 它前面），后两种是「**碰了，可能做了一半**」。用户据此决定要不要
/// [`undo_turn_force`](Session::undo_turn_force)。
///
/// 加的是成因**不是开关**：`ignore_undo_errors: bool` 那种事先设一次的参数等于
/// 替用户答了所有他还没被问到的问题，199 §五 明确否决过。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BlockedCause {
    /// 这一步没交还原函数——今天的屏障，**没碰**。
    NoHook,
    /// 钩子跑了但失败了——**碰了，可能做了一半**。
    HookFailed(Arc<str>),
    /// 钩子已随进程重启消失（`Hooked` 但表里查不到）。
    HookLost,
}

/// 逐条循环停在了哪里：已经退掉几条、卡在哪条 `seq`、为什么。
pub(super) struct HookStop {
    pub(super) undone: usize,
    pub(super) seq: u64,
    pub(super) cause: BlockedCause,
}

/// 这一次 undo 允不允许**越过一个**障碍（`/undo!` 的语义）。
///
/// 「一次确认放行一条」是 027 定下、199 §五 复核过的语义：用户看到的提示说的是
/// 「越过的是这一个 `shell/exec`」，那就只该越过这一个。放行全部等于让一次确认替
/// 用户答了几个他没被问到的问题。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Crossing {
    /// 普通 undo：撞上什么都停。
    Stop,
    /// `/undo!`：**在撤销的行进方向上遇到的第一个**障碍放行，之后照常停。
    ///
    /// 「行进方向上的第一个」是这一档的要害。`History` 那一侧的屏障谓词只看得见
    /// `&EntryMeta`、也只认得出 `Blocked`，所以它会乐观地放过第一条屏障；可屏障
    /// 可能比一条钩子失败的 entry **更老**，也就是排在后面。真正该被这一次确认
    /// 花掉的是先遇上的那个，所以额度的消费判定在这里的循环里，不在那个谓词里
    /// ——否则「屏障在下、钩子失败在上」时额度会花在够不着的屏障上，用户按多少次
    /// `/undo!` 都过不去那条失败的钩子。
    CrossOne,
}

impl Session {
    /// 逆序逐条：先跑钩子，`Ok` 才 `apply_prev`。
    ///
    /// 返回 `None` = 这一批全退掉了；`Some(stop)` = 停在中间，`stop.undone` 条已经
    /// 退掉、`stop.seq` 那一条**状态不回滚**（199 §五：碰了、可能做了一半，停在那一步）。
    ///
    /// # 为什么整个循环包在一个 `store.batch` 里
    ///
    /// 一次 undo 是一次状态跃迁，中间态不该被任何 derived 看见（`apply_prev` 的模块
    /// 文档）。逐条调 `apply_prev` 会变成逐条 flush：下游 derived 在「一半旧一半新」
    /// 的世界上重算若干次。外面套一层 batch 之后嵌套的 batch 只在最外层 flush 一次
    /// （`Store::batch` 的契约），于是「逐条交错」和「只冲一次」两件事同时成立。
    ///
    /// 钩子在这层 batch 里跑不影响它读到的东西：`Store::get` 只读不 flush，写入本身
    /// 在 `set_atom_state` 那一刻就已经生效，被推迟的只有传播。
    pub(super) fn unwind(
        &mut self,
        entries: &[AgentEntry],
        run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome,
        crossing: Crossing,
    ) -> Option<HookStop> {
        let mut allowance = matches!(crossing, Crossing::CrossOne);
        let mut stop = None;

        let (store, sources) = (self.store.clone(), self.sources.clone());
        let mut resolve = |key: &AtomKey| source_atom(&store, &sources, key);
        store.batch(|inner| {
            for (undone, entry) in entries.iter().enumerate() {
                if let Some(cause) = obstacle(entry, run_hook) {
                    // 额度只花在**行进方向上遇到的第一个**障碍上（见 `Crossing`）。
                    if !allowance {
                        stop = Some(HookStop {
                            undone,
                            seq: entry.seq,
                            cause,
                        });
                        return;
                    }
                    allowance = false;
                    // 越过 = 跳过这一步的**还原**，状态照退（199 §五 用户原话：
                    // 「状态就停在那一步，用户可以强制往回退，就等于跳过这一步」）。
                }
                self.rebuild_touched_agents(std::slice::from_ref(entry));
                apply_prev(inner, &mut resolve, std::slice::from_ref(entry));
            }
        });
        stop
    }

    /// 逐条循环停下之后，把游标推回**失败那一条的后面**。
    ///
    /// `History::undo_turn` 是先整批挪游标再把条目交出来的，可我们中途停了：后面
    /// 那些条目的 `prev` 一个字节都没写回去，游标却已经当它们退过了。不推回来就是
    /// 「日志说这些条目不算数、store 里它们的值还在」——正是红线 4/6 那一类
    /// 「不报错、只在下一次 undo/恢复时错值」的形状。
    ///
    /// 走 `redo_one` 而不是新开一个 `History` API：它做的正是「游标往前一格，把该
    /// 应用的条目交给我」，而这里**没有东西要应用**（那一条从来没被回滚过），所以
    /// 交回来的产物原地丢掉是对的，不是漏了一次应用。
    pub(super) fn recede_cursor(&mut self, entries: usize) {
        for _ in 0..entries {
            let _ = self.history.redo_one();
        }
    }
}

/// 这一条 entry 挡路吗？`None` = 可以退。
///
/// **只有 [`Undoability::Hooked`] 会去问钩子**：`StateOnly` 没碰外部世界，问了也
/// 没有意义——更要命的是，调用方的钩子表里本来就不会有它，问一次就会拿回一个
/// `Lost`，把一条本来干干净净的 entry 变成障碍。`Blocked` 进得到这里只有一种情况：
/// `/undo!` 那一侧的谓词已经放它进来了（见 [`Crossing::CrossOne`]）。
fn obstacle(
    entry: &AgentEntry,
    run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome,
) -> Option<BlockedCause> {
    match entry.meta.undoability {
        Undoability::StateOnly => None,
        Undoability::Blocked => Some(BlockedCause::NoHook),
        Undoability::Hooked => match run_hook(entry) {
            HookOutcome::Ok => None,
            HookOutcome::Failed(why) => Some(BlockedCause::HookFailed(why)),
            HookOutcome::Lost => Some(BlockedCause::HookLost),
        },
    }
}

#[cfg(test)]
#[path = "undo_hook_tests.rs"]
mod tests;
