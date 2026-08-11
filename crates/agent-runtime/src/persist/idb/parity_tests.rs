//! 114a 验收证据 #2 与 #3：
//!
//! - **#2**：同一段调用方代码（跟 `agent-runtime/tests/it/session_store_backend_choice.rs`
//!   的 `drive_session` 是同一份逻辑）分别喂 `Memory`（agent-store）、`Jsonl`（本
//!   crate）、`IdbStore<..,MemoryKv>`（本模块），三份 `load()` 的结果形状必须完全
//!   一致——这是「回放语义跟 `KvStore` 用哪个实现无关」这条设计要求的直接证据。
//! - **#3**（红线 11）：构造一份「关闭前最后一轮」的状态——`Slot::ToolsAllowed`
//!   持一份有序数组，跟生产代码的真实形状一致（见
//!   `agent_core::graph::slot::Slot::ToolsAllowed` 文档：「排序去重后落盘，它会被
//!   渲染进 prompt，顺序一漂前缀缓存就全价」）——存下来、重新 `load`、重放，断言
//!   的不是「长度一样」或「元素集合一样」，是**序列化出来的字节完全相同**。

use agent_store::SessionStore;
use agent_store::history::{Change, Entry, Snapshot};
use agent_store::persist::{LoadedSession, Memory};

use crate::Jsonl;
use crate::persist::PersistedMeta;

use super::{IdbStore, MemoryKv};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Val(i64);

fn entry(seq: u64, value: i64) -> Entry<String, Val, u32> {
    Entry {
        seq,
        meta: 7,
        changes: vec![Change {
            key: "a".to_string(),
            prev: Val(value - 1),
            next: Val(value),
        }],
    }
}

/// 跟 `session_store_backend_choice.rs::drive_session` 是同一段逻辑：写 3 条、落
/// 一张快照、再写 1 条，返回 `load()` 的结果。类型参数只要求 `SessionStore<..>`——
/// 这段代码不知道、也不需要知道自己在跟哪个后端打交道。
fn drive_session<S: SessionStore<String, Val, u32>>(backend: &S) -> LoadedSession<String, Val, u32> {
    for seq in 0..3 {
        backend.append(&entry(seq, seq as i64 + 1));
        backend.set_cursor(seq as usize + 1);
    }
    backend.snapshot(&Snapshot {
        values: vec![("a".to_string(), Val(3))],
    });
    backend.append(&entry(3, 4));
    backend.set_cursor(1);
    backend
        .load()
        .loaded()
        .expect("drive_session 写过东西之后 load 不该是 None")
}

/// 只比这几样：有没有快照、剩下哪些 seq（不比内容——`Val` 已经用 seq 唯一确定）、
/// cursor、next_seq。跟 `session_store_backend_choice.rs` 断言的东西一致。
fn shape(loaded: &LoadedSession<String, Val, u32>) -> (bool, Vec<u64>, usize, u64) {
    (
        loaded.snapshot.is_some(),
        loaded.entries.iter().map(|e| e.seq).collect(),
        loaded.cursor,
        loaded.next_seq,
    )
}

/// 验收证据 #2。
#[test]
fn write_load_replay_semantics_match_memory_and_jsonl() {
    let memory: Memory<String, Val, u32> = Memory::new();
    let from_memory = drive_session(&memory);

    let path = std::env::temp_dir().join(format!(
        "agent-runtime-idb-parity-{}.jsonl",
        std::process::id()
    ));
    let jsonl: Jsonl<String, Val, u32> = Jsonl::new(path.clone(), |_| {});
    let from_jsonl = drive_session(&jsonl);
    let _ = std::fs::remove_file(&path);

    let idb: IdbStore<String, Val, u32, MemoryKv> = IdbStore::spawn(MemoryKv::new(), |_| {});
    let from_idb = drive_session(&idb);

    let (m, j, i) = (shape(&from_memory), shape(&from_jsonl), shape(&from_idb));
    assert_eq!(
        m, j,
        "Memory 和 Jsonl 的重放结果形状先对不上——这条基准本身就没立住，IdbStore 没有可比对象"
    );
    assert_eq!(
        m, i,
        "IdbStore 的重放结果跟 Memory/Jsonl 形状不一致——回放语义分叉了，这正是 114a 的验收主证据"
    );
}

/// 验收证据 #3（红线 11）。
#[test]
fn tool_table_bytes_survive_persist_and_reload_byte_for_byte() {
    use agent_core::{AgentId, AgentValue, AtomKey, Slot};
    use std::sync::Arc;

    // 生产代码的真实形状：排序去重的工具名数组（Slot::ToolsAllowed 文档）。故意
    // 挑一个不按字母序也不按插入序「巧合排对」的顺序，让这条测试只有在实现真的
    // 保序时才会绿——如果 IdbStore 内部哪里不小心过了一遍 HashMap/HashSet，这里
    // 会红。
    let tools_before = serde_json::json!(["web:page/title", "mcp:crm/lookup", "srv:agent/spawn"]);

    let record: Entry<AtomKey, AgentValue, PersistedMeta> = Entry {
        seq: 0,
        meta: PersistedMeta {
            turn_id: 1,
            epoch: 0,
            label: "tool_result".to_string(),
            barrier: false,
        },
        changes: vec![Change {
            key: AtomKey::Agent(AgentId::root(), Slot::ToolsAllowed),
            prev: AgentValue::Null,
            next: AgentValue::Json(Arc::new(tools_before.clone())),
        }],
    };

    let store: IdbStore<AtomKey, AgentValue, PersistedMeta, MemoryKv> =
        IdbStore::spawn(MemoryKv::new(), |e| panic!("不该有加载错误：{e}"));
    store.append(&record);
    store.set_cursor(1);

    let loaded = store.load().loaded().expect("写过东西之后 load 不该是 None");
    assert_eq!(loaded.entries.len(), 1);
    let AgentValue::Json(replayed) = &loaded.entries[0].changes[0].next else {
        panic!("重放回来的值类型变了，不是 AgentValue::Json");
    };

    let before_bytes = serde_json::to_vec(&tools_before).unwrap();
    let after_bytes = serde_json::to_vec(replayed.as_ref()).unwrap();
    assert_eq!(
        before_bytes, after_bytes,
        "工具表数组经过 IndexedDB 持久化 + 重放之后字节变了——红线 11：前缀缓存靠逐字节相等，\
         次序漂一点就是每一轮都全价"
    );
}
