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
    /// - `on_unknown_key`：快照里有、这一版 schema 已经不认识的键（010 的
    ///   schema 演进：删掉的槽位），报给宿主，不静默丢。
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
            // 屏障恢复不需要这份列表（027 已裁决）：`barrier` 位随 `EntryMeta` 落盘，
            // `undo_turn` 读的是日志里的 `barrier`，不是这份运行时提示列表——它只在
            // **当次进程**里，工具结果落地的那一刻，把 `mark_irreversible` 登记过的
            // call_id 翻译成 entry 的 `barrier` 位，翻译一旦发生就不再需要了。
            irreversible: Vec::new(),
            // 结构性硬限是**配置**不是状态（`Session` 的字段表），落盘里没有它。
            // 宿主要非默认值，恢复后调 `set_agent_limits` —— 和 `history_cap`
            // 一样是「载入后重调」的东西。
            limits: crate::command::spawn::AgentLimits::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::meta::EntryMeta;
    use crate::engine::state::TurnStatus;
    use crate::graph::Slot;
    use agent_store::Change;

    fn agent() -> AgentId {
        AgentId::root()
    }

    fn meta(turn_id: u64, epoch: u64, label: &'static str) -> EntryMeta {
        EntryMeta { turn_id, epoch: Epoch(epoch), label, barrier: false }
    }

    fn status_change(prev: TurnStatus, next: TurnStatus) -> Change<AtomKey, AgentValue> {
        Change {
            key: AtomKey::Agent(agent(), Slot::Status),
            prev: AgentValue::Status(prev),
            next: AgentValue::Status(next),
        }
    }

    /// 没有快照、entries 全部生效（cursor == len）：等价于「从头整份重放」。
    #[test]
    fn no_snapshot_replays_every_entry_up_to_cursor() {
        let entries = vec![AgentEntry {
            seq: 0,
            meta: meta(1, 0, "user_input"),
            changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
        }];
        let mut unknown = Vec::new();
        let session = Session::restore(agent(), None, entries, 1, 1, 100, &mut |k| unknown.push(k.clone())).unwrap();

        assert_eq!(session.status(), TurnStatus::Thinking);
        assert_eq!(session.turn_id(), 1);
        assert_eq!(session.epoch(), Epoch(1));
        assert!(unknown.is_empty());
    }

    /// 游标不在栈顶：`[cursor, len)` 是 redo 尾，**不写回** store，但仍然留在
    /// `History` 里，`redo_turn` 应该能把它找回来。
    #[test]
    fn entries_past_the_cursor_are_not_replayed_but_stay_redoable() {
        let entries = vec![
            AgentEntry {
                seq: 0,
                meta: meta(1, 0, "user_input"),
                changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
            },
            AgentEntry {
                seq: 1,
                meta: meta(1, 0, "cancel"),
                changes: vec![status_change(
                    TurnStatus::Thinking,
                    TurnStatus::Failed(crate::engine::state::Failure::Cancelled),
                )],
            },
        ];
        let mut unknown = Vec::new();
        let mut session =
            Session::restore(agent(), None, entries, 1, 2, 100, &mut |k| unknown.push(k.clone())).unwrap();

        // 只应用了第一条：状态是 Thinking，不是 Cancelled。
        assert_eq!(session.status(), TurnStatus::Thinking);
        assert_eq!(session.cursor(), 1);
        assert_eq!(session.history_len(), 2);

        // redo 能把第二条找回来——它没有丢，只是没被应用。
        let report = session.redo_turn();
        assert!(matches!(report, crate::command::UndoReport::Applied { entries: 1, .. }));
        assert_eq!(session.status(), TurnStatus::Failed(crate::engine::state::Failure::Cancelled));
    }

    /// 快照 + 之后的日志：快照灌回 primitive，日志接着把状态推到快照点之后。
    #[test]
    fn a_snapshot_seeds_primitives_then_entries_advance_past_it() {
        let snapshot = vec![(AtomKey::Agent(agent(), Slot::Status), AgentValue::Status(TurnStatus::Thinking))];
        let entries = vec![AgentEntry {
            seq: 5,
            meta: meta(3, 2, "provider_failed"),
            changes: vec![status_change(
                TurnStatus::Thinking,
                TurnStatus::Failed(crate::engine::state::Failure::Provider(crate::seam::ErrorClass::Unknown)),
            )],
        }];
        let session =
            Session::restore(agent(), Some(snapshot), entries, 1, 6, 100, &mut |_| panic!("不该有不认识的键")).unwrap();

        assert_eq!(session.turn_id(), 3);
        assert_eq!(session.epoch(), Epoch(3));
        assert!(matches!(session.status(), TurnStatus::Failed(_)));
    }

    /// 快照里有一个这一版 schema 已经不认识的键——`on_unknown_key` 收到，不 panic，
    /// 其余照常灌回。
    #[test]
    fn an_unknown_snapshot_key_is_reported_not_silently_dropped() {
        let dropped_key = AtomKey::ToolCall(agent(), crate::ids::ToolCallId::new("gone"), crate::graph::ToolCallSlot::Result);
        let snapshot = vec![
            (AtomKey::Agent(agent(), Slot::Status), AgentValue::Status(TurnStatus::Idle)),
            (dropped_key.clone(), AgentValue::Text(std::sync::Arc::from("旧版本的东西"))),
        ];
        let mut unknown = Vec::new();
        let session = Session::restore(agent(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| unknown.push(k.clone())).unwrap();

        assert_eq!(unknown, vec![dropped_key]);
        assert_eq!(session.status(), TurnStatus::Idle);
        assert_eq!(session.turn_id(), 1); // 没有 entry，退回起点
        assert_eq!(session.epoch(), Epoch::START);
    }

    /// 破坏 `History::from_parts` 不变量的落盘件原样拒绝，不硬凑。
    #[test]
    fn invalid_persisted_history_is_rejected() {
        let entries = vec![AgentEntry {
            seq: 0,
            meta: meta(1, 0, "user_input"),
            changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
        }];
        let Err(err) = Session::restore(agent(), None, entries, 5 /* 越界 */, 1, 100, &mut |_| {}) else {
            panic!("越界游标该被拒绝");
        };
        assert_eq!(err, InvalidHistory::CursorOutOfRange);
    }
}
