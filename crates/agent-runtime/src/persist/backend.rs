//! 挑后端：有路径落 [`Jsonl`]，没有落 [`Memory`]（027 决策：临时会话用 `Memory`，
//! 「无则 Memory」不是「不持久化就没有 store」，是「换一个不做 IO 的实现」——两条路
//! 都走同一套 [`sync`](super::sync::sync)/[`recover`](super::recover::recover) 管道，
//! CLI 不需要为「有没有 `--session`」分叉逻辑。

use std::path::PathBuf;

use agent_store::persist::Memory;

use crate::Jsonl;
use crate::jsonl::SessionStoreError;

use super::{PersistedMeta, SessionBackend};

/// `path` 为 `Some` 落 `Jsonl`（真文件），`None` 落 `Memory`（进程退出即丢）。
/// `on_error` 只有 `Jsonl` 会用到（`Memory` 零 IO，不会失败）。
pub fn open_backend(
    path: Option<PathBuf>,
    on_error: impl Fn(SessionStoreError) + Send + Sync + 'static,
) -> Box<SessionBackend> {
    match path {
        Some(p) => Box::new(Jsonl::<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta>::new(p, on_error)),
        None => Box::new(Memory::<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta>::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_path_opens_a_memory_backend_that_starts_empty() {
        let store = open_backend(None, |_| {});
        assert!(store.load().is_absent());
    }

    #[test]
    fn a_path_opens_a_jsonl_backend_writable_and_loadable() {
        let path = std::env::temp_dir().join(format!("agent-runtime-open-backend-{}.jsonl", std::process::id()));
        let store = open_backend(Some(path.clone()), |_| {});
        let entry = agent_store::Entry {
            seq: 0,
            meta: PersistedMeta { turn_id: 1, epoch: 0, label: "user_input".to_string(), barrier: false },
            changes: vec![agent_store::Change {
                key: agent_core::AtomKey::Agent(agent_core::AgentId::root(), agent_core::Slot::TurnsUsed),
                prev: agent_core::AgentValue::U64(0),
                next: agent_core::AgentValue::U64(1),
            }],
        };
        store.append(&entry);
        store.set_cursor(1);
        let loaded = store.load().loaded().expect("写过东西该能载回来");
        assert_eq!(loaded.entries.len(), 1);
        let _ = std::fs::remove_file(&path);
    }
}
