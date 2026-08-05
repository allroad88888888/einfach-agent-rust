//! 每条命令之后把 [`Session`] 的变化转发进 [`SessionStore`]（011 的端口在这里上岗）。
//!
//! ## 调用契约：`RedoTail` 必须在新条目之前转发，`Oldest` 必须在之后
//!
//! 011 的实做记录写的是「append+set_cursor 之后再转发裁剪事件」，但这条顺序对
//! `DropEvent::RedoTail` 是错的，会真的丢数据——本文件的测试
//! `overwriting_a_redo_tail_does_not_resend_seqs_already_told_to_the_store` 就是
//! 在实现这一条时当场炸出来的：
//!
//! - `SessionLog::record_drop_after(first_seq, _)` 的实现是
//!   `held.retain(|e| e.seq < first_seq)`——一个绝对阈值。`first_seq` 是被覆盖掉的
//!   redo 尾里最早一条的 seq，而新写入的那条 entry 的 seq **必然大于**它（seq 只增
//!   不减，新 entry 是在截断redo尾之后才铸的号）。所以如果**先把新 entry append 进
//!   store，再转发这条 `RedoTail`**，`retain` 会把刚刚追加的新 entry 一并冲掉——
//!   `agent-store/tests/session_log_replay.rs::drop_after_only_trims_the_tail_and_
//!   leaves_the_front_offset_untouched` 测的顺序反过来就是先 `record_drop_after`
//!   再 `record_append`，这才是唯一对的顺序。
//! - `DropEvent::Oldest`（cap 驱逐）没有这个问题——`History::enforce_cap` 本身是在
//!   一次 `append` **完成之后**才跑的（新条目已经在 `entries` 里，可能连它自己都在
//!   驱逐范围内），所以先 append 再转发 `Oldest` 才是跟 `History` 内部顺序一致的
//!   那一个。
//!
//! 于是这个函数按事件种类分两段转发：`RedoTail` 在追新条目**之前**、`Oldest` 在
//! **之后**，`set_cursor` 摆在追加新条目之后（与 011 那条没有冲突的部分一致）。
//!
//! ## 为什么按 `seq` 高水位追新条目，不是按下标数
//!
//! `Session::history()` 每次读到的是**当前**的 entries 列表——undo 之后又写新内容会
//! truncate 掉 redo 尾（下标随之整体前移），cap 驱逐会从最老一端切掉（下标也会前移）。
//! 按下标数「上次同步到第几条」在这两种情况下都会算错。`seq` 不一样：它全局单调、
//! 一旦铸出来就不会有第二条 entry 用同一个号，也不会因为前面的条目被裁掉而回收
//! ——所以「这个 seq 有没有被我告诉过 store」是一个跟下标无关、只增不减的判断，
//! [`RunnerCtx`] 只需要记一个「目前为止同步到的最大 seq」（[`RunnerCtx::persisted_seq`]），
//! `Vec<Change>` 级的下标位移完全不用管。

use agent_core::Session;
use agent_store::DropEvent;

use crate::ctx::RunnerCtx;

use super::meta::PersistedMeta;

/// 转发这一次命令的全部效果：先转发 `RedoTail`（如果有），再 append 新条目并
/// `set_cursor`，最后转发 `Oldest`（如果有）。**每条命令之后调一次**——`run_turn`
/// 内部每个 `session.step(..)` 之后调；`begin_turn`/`undo_turn`/`redo_turn`/
/// `undo_turn_force`/`set_max_turns` 这些 CLI 直接调用的会话命令，调用方必须在
/// 调用之后也叫一次（它们同样会挪游标、undo/redo 甚至不产生新条目，但游标动了
/// 就必须 `set_cursor`）。
/// 恢复之后必须调一次：把 `persisted_seq` 这个同步水位对齐到 `session` 里
/// 已经有的 entries——`Session::restore` 灌回来的整段 history 全部来自
/// `session_store` 自己（`persist::recover` 读出来又喂回去的同一批数据），
/// `sync` 不该把它们当成新增量再 append 一遍。
///
/// ## 真 bug：忘了这一步会把会话文件永久搞坏
///
/// `RunnerCtx::new` 的 `persisted_seq` 起手式是 `None`——对全新会话（从来没
/// 写过任何东西的 `SessionStore`）这是对的：还没告诉过 store 任何 seq。但
/// 恢复路径不是「全新」：`recover()` 已经把整段 history 从 store 读回来重建
/// 进了 `session`，如果 `persisted_seq` 还停在 `None`，下一次 `sync()` 会把
/// `session.history().entries()` 里那些**本来就在盘上**的旧条目全部当成
/// 「从未同步过」，重新 append 一遍——4 行的单轮文件长到 12 行，`seq` 在
/// 文件中段跌回 0，下一次启动 `History::from_parts` 撞 `SeqNotIncreasing`
/// 硬失败，会话彻底搁浅（连续「起会话→写一轮→重启」周期精确复现，回归测试
/// 见 `agent-cli/tests/indep_restart_continue.rs` 与
/// `agent-runtime/tests/jsonl_restart_continues.rs`）。
///
/// 对全新会话（`session.history().entries()` 为空）调用这个函数是无害的
/// 空操作——调用方不需要在「这次是不是恢复出来的」上分支，构造完 `ctx`
/// 之后不管走哪条路径都调用一次即可，恒等式自然成立。
pub fn seed_after_recover(ctx: &mut RunnerCtx, session: &Session) {
    ctx.persisted_seq = session.history().entries().map(|e| e.seq).max();
}

pub fn sync(ctx: &mut RunnerCtx, session: &mut Session) {
    let drop_events = session.take_drop_events();

    for ev in &drop_events {
        if let DropEvent::RedoTail { first_seq, count } = ev {
            ctx.session_store.drop_after(*first_seq, *count);
        }
    }

    for entry in session.history().entries() {
        let already_synced = ctx.persisted_seq.is_some_and(|s| entry.seq <= s);
        if already_synced {
            continue;
        }
        let persisted = agent_store::Entry {
            seq: entry.seq,
            meta: PersistedMeta::from(&entry.meta),
            changes: entry.changes.clone(),
        };
        ctx.session_store.append(&persisted);
        ctx.persisted_seq = Some(entry.seq);
    }

    ctx.session_store.set_cursor(session.cursor());

    for ev in drop_events {
        if let DropEvent::Oldest { count } = ev {
            ctx.session_store.drop_oldest(count);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{AgentId, Event};
    use agent_store::persist::Memory;

    use super::*;
    use crate::persist::SessionBackend;
    use crate::tool_table::ToolTable;
    use agent_providers::deepseek::DeepSeek;
    use agent_tools::ToolExecutor;
    use agent_transport::Client;

    fn ctx() -> RunnerCtx {
        let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
        let store: Box<SessionBackend> = Box::new(Memory::new());
        RunnerCtx::new(
            Arc::new(DeepSeek),
            Arc::new(Client::new()),
            "https://api.deepseek.com/chat/completions".to_string(),
            "key".to_string(),
            fs,
            ToolTable::builtin(),
            Vec::new(),
            agent_core::SessionConfig {
                model: Arc::from("m"),
                temperature: None,
                max_tokens: None,
                context_window: None,
            },
            store,
            Box::new(|_| {}),
        )
    }

    /// 一次 `step` 产生一条 entry → sync 之后 store 里正好有它，游标对得上。
    #[test]
    fn a_new_entry_is_appended_and_the_cursor_follows() {
        let mut ctx = ctx();
        let mut session = Session::new(AgentId::root());

        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("hi"),
            images: Vec::new(),
        });
        sync(&mut ctx, &mut session);

        let loaded = ctx
            .session_store
            .load()
            .loaded()
            .expect("写过东西不该是 None");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.cursor, 1);
        assert_eq!(ctx.persisted_seq, Some(0));
    }

    /// undo 不产生新 entry，但游标退了——sync 之后 store 的游标必须跟着退，
    /// 而不是停在上一次同步时的值。
    #[test]
    fn undo_moves_the_cursor_without_a_new_entry_and_sync_still_forwards_it() {
        let mut ctx = ctx();
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("hi"),
            images: Vec::new(),
        });
        sync(&mut ctx, &mut session);

        let _ = session.undo_turn();
        sync(&mut ctx, &mut session);

        let loaded = ctx.session_store.load().loaded().unwrap();
        assert_eq!(loaded.cursor, 0, "undo 之后游标该退到 0，即使没有新 entry");
        assert_eq!(loaded.entries.len(), 1, "entry 本身还在，只是不再生效");
    }

    /// 覆盖 redo 尾之后再写新内容：旧 seq 不会被重新 append（`persisted_seq`
    /// 按 seq 高水位判断，不受下标位移影响），新 entry 真的落进了 store——
    /// 这是本文件模块文档记的那个顺序 bug 的回归测试：先转发 `RedoTail` 再
    /// append 新条目，新条目才不会被那条 `retain` 误杀。
    #[test]
    fn overwriting_a_redo_tail_does_not_resend_seqs_already_told_to_the_store() {
        let mut ctx = ctx();
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("first"),
            images: Vec::new(),
        });
        sync(&mut ctx, &mut session);
        let _ = session.undo_turn();
        sync(&mut ctx, &mut session);

        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("second"),
            images: Vec::new(),
        });
        sync(&mut ctx, &mut session);

        let loaded = ctx.session_store.load().loaded().unwrap();
        assert_eq!(
            loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(loaded.cursor, 1);
    }
}
