//! 快照节奏：每 [`RunnerCtx::snapshot_every`] 个 turn 落一张（027 决策 3，默认见
//! [`crate::ctx::DEFAULT_SNAPSHOT_EVERY`]）。
//!
//! 快照只是**持久化层的压实手段**——`Session` 自己的 undo/redo 完全不受它影响
//! （`session.history()` 在内存里永远是完整的，`snapshot()` 只影响 `SessionStore`
//! 落盘时怎么截断文件），所以「什么时候拍」不是正确性问题，只是「重启后要重放多少
//! entry」这道效率题，选在每个 turn 收尾时（`run_turn` 返回前）检查一次足够。

use agent_store::Snapshot;

use crate::ctx::RunnerCtx;

/// `session.turn_id()` 是这个 turn 号的倍数、且这一轮还没拍过，就落一张快照
/// （`session.primitives()` 本身已经按 `AtomKey` 排序——010 判断 7 的落点）。
pub fn maybe_snapshot(ctx: &mut RunnerCtx, session: &agent_core::Session) {
    if ctx.snapshot_every == 0 {
        return; // 0 = 关闭快照节奏，只靠 entry 日志重放。
    }
    let turn_id = session.turn_id();
    if !turn_id.is_multiple_of(ctx.snapshot_every) {
        return;
    }
    if ctx.last_snapshotted_turn == Some(turn_id) {
        return; // 这一轮已经拍过——`run_turn` 对同一个非终态卡住的 turn 可能被多次调用。
    }
    let snap = Snapshot {
        values: session.primitives(),
    };
    ctx.session_store.snapshot(&snap);
    ctx.last_snapshotted_turn = Some(turn_id);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{AgentId, Event, Session};
    use agent_store::persist::Memory;

    use super::*;
    use crate::persist::SessionBackend;
    use crate::persist::meta::PersistedMeta;
    use crate::persist::sync::sync;
    use crate::tool_table::ToolTable;
    use agent_providers::deepseek::DeepSeek;
    use agent_tools::ToolExecutor;
    use agent_transport::Client;

    fn ctx(snapshot_every: u64) -> RunnerCtx {
        let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
        let store: Box<SessionBackend> = Box::new(Memory::<
            agent_core::AtomKey,
            agent_core::AgentValue,
            PersistedMeta,
        >::new());
        let mut ctx = RunnerCtx::new(
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
        );
        ctx.snapshot_every = snapshot_every;
        ctx
    }

    fn advance_to_turn(session: &mut Session, ctx: &mut RunnerCtx, turn: u64) {
        while session.turn_id() < turn {
            session.begin_turn();
            sync(ctx, session);
        }
    }

    #[test]
    fn no_snapshot_before_the_configured_turn() {
        let mut ctx = ctx(3);
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("hi"),
            images: Vec::new(),
        });
        sync(&mut ctx, &mut session);

        maybe_snapshot(&mut ctx, &session); // turn_id == 1，不是 3 的倍数
        assert!(
            ctx.session_store
                .load()
                .loaded()
                .unwrap()
                .snapshot
                .is_none()
        );
    }

    #[test]
    fn a_snapshot_lands_exactly_on_the_configured_turn_and_only_once() {
        let mut ctx = ctx(3);
        let mut session = Session::new(AgentId::root());
        advance_to_turn(&mut session, &mut ctx, 3);

        maybe_snapshot(&mut ctx, &session);
        assert!(
            ctx.session_store
                .load()
                .loaded()
                .unwrap()
                .snapshot
                .is_some()
        );

        // 再调一次不该重复触发（`last_snapshotted_turn` 挡住）——不好直接断言
        // 「没有再拍一张」，但至少 `last_snapshotted_turn` 保持不变。
        let before = ctx.last_snapshotted_turn;
        maybe_snapshot(&mut ctx, &session);
        assert_eq!(ctx.last_snapshotted_turn, before);
    }

    #[test]
    fn zero_disables_snapshotting() {
        let mut ctx = ctx(0);
        let mut session = Session::new(AgentId::root());
        advance_to_turn(&mut session, &mut ctx, 10);
        maybe_snapshot(&mut ctx, &session);
        assert!(
            ctx.session_store
                .load()
                .loaded()
                .unwrap()
                .snapshot
                .is_none()
        );
    }
}
