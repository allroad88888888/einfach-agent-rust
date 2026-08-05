//! 011 验收「可 per-session 选后端：临时会话 Memory，重要会话落盘」——同一段调用方
//! 代码（一个泛型函数，只认 `SessionStore<K, V, M>`，不知道自己在跟哪个实现打交道）
//! 分别喂 `Memory`（agent-store）和 `Jsonl`（本 crate），两边跑出同样形状的结果。
//!
//! 这是两个后端唯一都在场的地方——`Memory` 住 agent-store（不能反向依赖
//! agent-runtime），所以「同一段调用方代码换后端」这条验收只能在这个 crate 的测试里
//! 做完整版本；agent-store 自己的 `session_store_memory_full_chain.rs` 只测了
//! `Memory` 单独那一半。

mod session_store_support;

use agent_store::history::{Change, Entry};
use agent_store::{Memory, SessionStore, Snapshot};

use agent_runtime::Jsonl;
use session_store_support::{Val, collecting_on_error, temp_path};

fn entry(seq: u64, value: i64) -> Entry<String, Val, u32> {
    Entry { seq, meta: 7, changes: vec![Change { key: "a".to_string(), prev: Val(value - 1), next: Val(value) }] }
}

/// 唯一一份调用方逻辑：写 3 条、落一张快照、再写 1 条，返回 `load()` 的结果。
/// 类型参数只要求 `SessionStore<..>`——这段代码本身不知道、也不需要知道自己在跟
/// `Memory` 还是 `Jsonl`打交道。
fn drive_session<S: SessionStore<String, Val, u32>>(backend: &S) -> agent_store::persist::LoadedSession<String, Val, u32> {
    for seq in 0..3 {
        backend.append(&entry(seq, seq as i64 + 1));
        backend.set_cursor(seq as usize + 1);
    }
    backend.snapshot(&Snapshot { values: vec![("a".to_string(), Val(3))] });
    backend.append(&entry(3, 4));
    backend.set_cursor(1);
    backend.load().loaded().expect("drive_session 写过东西之后 load 不该是 None")
}

#[test]
fn the_same_generic_call_site_produces_the_same_shape_for_both_backends() {
    let memory: Memory<String, Val, u32> = Memory::new();
    let from_memory = drive_session(&memory);

    let (errors, on_error) = collecting_on_error();
    let jsonl: Jsonl<String, Val, u32> = Jsonl::new(temp_path("backend-choice"), on_error);
    let from_jsonl = drive_session(&jsonl);
    assert!(errors.lock().unwrap().is_empty());

    assert_eq!(from_memory.snapshot.is_some(), from_jsonl.snapshot.is_some());
    assert_eq!(
        from_memory.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        from_jsonl.entries.iter().map(|e| e.seq).collect::<Vec<_>>()
    );
    assert_eq!(from_memory.cursor, from_jsonl.cursor);
    assert_eq!(from_memory.next_seq, from_jsonl.next_seq);
}

/// 「临时会话 Memory，重要会话落盘」的字面意思：同一个宿主类型（这里用
/// `Box<dyn SessionStore<..>>` 表达「运行时按会话选一个」）能同时装下两种后端。
#[test]
fn per_session_the_backend_can_be_chosen_at_construction_time() {
    let (_errors, on_error) = collecting_on_error();
    let backends: Vec<Box<dyn SessionStore<String, Val, u32>>> = vec![
        Box::new(Memory::<String, Val, u32>::new()),
        Box::new(Jsonl::<String, Val, u32>::new(temp_path("backend-choice-dyn"), on_error)),
    ];
    for backend in &backends {
        backend.append(&entry(0, 1));
        backend.set_cursor(1);
        assert_eq!(backend.load().loaded().unwrap().entries.len(), 1);
    }
}
