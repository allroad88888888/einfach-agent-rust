//! 崩溃恢复：从持久化产物重建一个 [`Session`]。**恢复就是 redo**（010）——
//! 构图、灌回快照、沿 `apply_next` 推进到游标位置，不写第二套加载逻辑。
//!
//! 这是 027 的落点：`SessionStore::load()`（011）给出 `(Option<Snapshot>,
//! Vec<Entry>, cursor, next_seq)`，宿主把 `Entry` 的 `M`（落盘时是 `String` label
//! 的持久化格式）翻译成 [`EntryMeta`]（`known_label` 就是那张翻译表），再调这里的
//! [`Session::restore`]。

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use agent_store::{AtomFamily, History, InvalidHistory, Snapshot, Store};

use crate::engine::epoch::Epoch;
use crate::graph::{AgentStore, AtomKey, DerivedFamily, SourceFamily, build_agent, source_atom};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

use super::meta::AgentEntry;
use super::session::Session;
use super::spawn::AgentLimits;

impl Session {
    /// 重建一个会话。
    ///
    /// - `snapshot`：`None` = 这个会话从没落过快照（`load()` 直接从头重放全部日志）；
    ///   `Some(values)` 先灌回这些 primitive，`entries` 只是快照点之后的那一段
    ///   （011 的 `LoadedSession` 契约）。
    /// - `entries` / `cursor` / `next_seq`：`SessionStore::load()` 的产物，`cursor`
    ///   是「这些 entries 里有几条已生效」——`[0, cursor)` 才会被写回 store，
    ///   `[cursor, entries.len())` 是被 undo 掉、还能 `redo_turn` 找回来的尾巴，
    ///   **不写回**（写回就是把一次已经放弃的 undo 悄悄恢复，不诚实）。
    /// - `history_cap`：载入后要重调的日志上限（011 的推给 027：`from_parts` 出来的
    ///   日志天生无 cap）。传 [`super::session::DEFAULT_HISTORY_CAP`] 就是默认档。
    /// - `limits`：载入后要重调的结构性硬限（决策 20 的两道闸）。**和 `history_cap`
    ///   同一类东西**——是这个会话的配置、不进原子图也不进日志，所以恢复不出来，
    ///   必须由宿主把它这一侧的那一份**再说一遍**。传 [`AgentLimits::default`]
    ///   就是决策 20 的默认档（深度 ≤3、子数 ≤8）。
    /// - `on_unknown_key`：快照里有、这一版 schema 已经不认识的键（010 的
    ///   schema 演进：删掉的槽位），报给宿主，不静默丢。
    ///
    /// # `limits` 为什么必须是入参（160）
    ///
    /// 它曾经在这里被硬写成 [`AgentLimits::default`]，注释还写着「宿主要非默认值，
    /// 恢复后调 `set_agent_limits`」——**可那时候根本没有这个入参**，`recover` 也没
    /// 转发通道，宿主想重调也无从下手。今天配置值恒等于默认值，两边永远相等，所以
    /// 这个洞看不出来；上限一可配（161/162 的 flag），第一次重启就显形：闸退回 8，
    /// 而工具描述里还写着部署方配的那个数，**模型按看到的数字规划，然后撞上一道它
    /// 无法预见的墙**。两侧数字必须是同一组，这是 `ToolTable::with_spawn` 与
    /// `registry::spec::ToolTableSpec` 反复记着的耦合，恢复路径也不例外。
    ///
    /// # 校验失败原样返回
    ///
    /// 落盘的 `entries`/`cursor`/`next_seq` 不满足 `History::from_parts` 的三条不变量
    /// （游标越界 / seq 不严格递增 / `next_seq` 太小）就是文件损坏或版本不兼容，
    /// 拒绝恢复、把错误交还给宿主决定怎么办（`docs/issues/011-session-store.md`
    /// 的诚实原则：不硬凑一个能跑的假状态）。
    ///
    /// # epoch / turn_id：从日志取能取到的最大值
    ///
    /// 两者都不进原子图（026 判断 4），快照里没有它们的踪迹。`turn_id` 取
    /// `entries` 里出现过的最大值（会话仍然逻辑上停在那一轮，下一次
    /// [`Session::begin_turn`] 才会翻页）；没有任何 entry（快照正好落在最新状态、
    /// 后面什么都没发生过）就退回 1，跟 [`Session::new`] 的起点一致。
    ///
    /// `epoch` 取「见过的最大值 + 1」——**不是**精确复原崩溃前那一刻的真实世代
    /// （比如 undo/redo 本身会 bump 世代但不产生 entry，那一下的凭证随进程一起
    /// 没了）。这个近似是安全的：世代号唯一的作用是拦「在飞 effect 的过期回执」
    /// （红线 6），而进程重启之后**不可能还有旧进程的在飞 effect**——没有东西可拦，
    /// 选哪个值都不影响正确性，选「见过的最大值 + 1」只是图一个干净、单调、
    /// 不会撞见旧记录的起点。
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        agent: AgentId,
        snapshot: Option<Vec<(AtomKey, AgentValue)>>,
        entries: Vec<AgentEntry>,
        cursor: usize,
        next_seq: u64,
        history_cap: usize,
        limits: AgentLimits,
        on_unknown_key: &mut impl FnMut(&AtomKey),
    ) -> Result<Session, InvalidHistory> {
        let max_epoch = entries.iter().map(|e| e.meta.epoch).max();
        let max_turn = entries.iter().map(|e| e.meta.turn_id).max();

        let mut history = History::from_parts(entries, cursor, next_seq)?;
        history.set_cap(Some(history_cap));

        let store: AgentStore = Store::new();
        let sources: SourceFamily = Rc::new(RefCell::new(AtomFamily::new()));
        let derived: DerivedFamily = Rc::new(RefCell::new(AtomFamily::new()));

        // 只有「已生效」的那一段（[0, cursor)）写回 store——`[cursor, len)` 是被
        // undo 掉还没 redo 回来的尾巴，写回去就是替用户悄悄撤销一次他做过的 undo。
        let in_effect: Vec<AgentEntry> = history.entries().take(cursor).cloned().collect();

        // 028：建的是**整棵树**。「这个会话当时有哪些 agent」不用另存一份名单——
        // 它就写在落盘的键上（红线 4 用逻辑键换来的红利）。redo 尾里的 agent 此刻
        // 还不存在，真被 redo 回来时 `undo.rs` 的 `rebuild_touched_agents` 会补。
        let mut agents: BTreeSet<AgentId> = BTreeSet::new();
        agents.insert(agent.clone());
        if let Some(values) = &snapshot {
            agents.extend(values.iter().map(|(key, _)| key.agent().clone()));
        }
        agents.extend(
            in_effect
                .iter()
                .flat_map(|entry| entry.changes.iter())
                .map(|change| change.key.agent().clone()),
        );
        for a in &agents {
            build_agent(&store, &sources, &derived, a);
        }

        if let Some(values) = snapshot {
            let snap = Snapshot { values };
            // 非创建查找（010 判断 5）：快照里的键要是这一版 schema 已经不认识，
            // 必须能说「不认识」而不是凭空造一个没人读的 atom。
            let mut resolve = |key: &AtomKey| sources.borrow().get(key);
            agent_store::restore(&store, &mut resolve, &snap, on_unknown_key);
        }

        // get-or-create（019 的重建路径，与命令层写入、applier 是同一行代码）。
        let mut resolve = |key: &AtomKey| source_atom(&store, &sources, key);
        agent_store::apply_next(&store, &mut resolve, &in_effect);

        Ok(Session {
            agent,
            store,
            sources,
            derived,
            history,
            epoch: Epoch(max_epoch.map_or(0, |e| e.0 + 1)),
            turn_id: max_turn.unwrap_or(1),
            // 屏障恢复不需要这份列表（027 已裁决）：档位随 `EntryMeta.undoability`
            // 落盘，`undo_turn` 读的是日志里的那一位，不是这份运行时提示列表——它只在
            // **当次进程**里，工具结果落地的那一刻，把 `mark_no_undo` /
            // `mark_hooked` 登记过的 call_id 翻译成 entry 的档位，翻译一旦发生就
            // 不再需要了。**`Hooked` 那一档的还原函数本身另说**：它是闭包、住 runtime、
            // 不跨进程，恢复之后钩子表是空的，撞上就按「钩子已消失」处理（199 §九）。
            tool_marks: Vec::new(),
            // 结构性硬限是**配置**不是状态（`Session` 的字段表），落盘里没有它——
            // 所以它恢复不出来，只能由宿主经入参**再说一遍**（160；本函数文档
            // 「`limits` 为什么必须是入参」记了它曾经硬写 default 埋下的静默失配）。
            // 和 `history_cap` 同一类，两者在参数表里也排在一起。
            limits,
        })
    }
}

/// 白盒单测拆去 `restore_tests.rs`（这个文件已经顶到红线 9 的 300 行，而 160
/// 还要往 `Session::restore` 上加参数）——同 `spawn.rs`/`despawn.rs` 的先例。
#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
