//! `AgentValue` 的贴身单元测试。放 `tests/` 而不是内联 `#[cfg(test)] mod tests`：
//! 红线 9 写明「本仓的取向是把集成测试挪到 tests/，源文件里只留最贴身的单元测试」，
//! 而 `atom_value.rs` 的实现区加上这批断言会顶破 300 行上限。用到的类型全部是
//! `agent_core` 的公开类型，挪过来零损失可见性。

use std::sync::Arc;

use imbl::Vector;

use agent_core::engine::state::{ToolSlot, TurnStatus};
use agent_core::seam::PrefixImage;
use agent_core::ids::{MessageId, ToolCallId};
use agent_core::value::atom_value::AgentValue;
use agent_core::value::message::{ContentBlock, Message, Role};

fn msg(id: u64) -> Message {
    Message {
        id: MessageId(id),
        role: Role::User,
        blocks: vec![ContentBlock::Text(Arc::from("hi"))],
    }
}

fn slots(n: usize) -> Arc<Vec<ToolSlot>> {
    Arc::new(
        (0..n)
            .map(|i| ToolSlot {
                call_id: ToolCallId::new(format!("call_{i}")),
                tool: Arc::from("srv:fs/read"),
                input: Arc::new(serde_json::json!({})),
                state: agent_core::SlotState::Pending,
            })
            .collect(),
    )
}

/// 同一个 `Arc` 的两份克隆走指针快路；内容相同但分配不同的两份走深比较，
/// **答案必须一样**——`ptr_eq` 是快路不是语义。
#[test]
fn arc_variants_compare_equal_through_both_the_pointer_path_and_the_deep_path() {
    let shared: Arc<str> = Arc::from("hello");
    assert_eq!(AgentValue::Text(shared.clone()), AgentValue::Text(shared));
    assert_eq!(
        AgentValue::Text(Arc::from("hello")),
        AgentValue::Text(Arc::from("hello"))
    );
    assert_ne!(
        AgentValue::Text(Arc::from("hello")),
        AgentValue::Text(Arc::from("world"))
    );

    let j = Arc::new(serde_json::json!({"a": 1}));
    assert_eq!(AgentValue::Json(j.clone()), AgentValue::Json(j));
    assert_eq!(
        AgentValue::Json(Arc::new(serde_json::json!({"a": 1}))),
        AgentValue::Json(Arc::new(serde_json::json!({"a": 1})))
    );

    let s = slots(2);
    assert_eq!(AgentValue::Slots(s.clone()), AgentValue::Slots(s));
    assert_eq!(AgentValue::Slots(slots(2)), AgentValue::Slots(slots(2)));
    assert_ne!(AgentValue::Slots(slots(2)), AgentValue::Slots(slots(3)));
}

/// `Messages` 的两条路：克隆共享堆上的块（`ptr_eq` 真，走快路）、push 之后块不同
/// （落深比较，答案是「不等」，正确）。结构共享让「undo 日志里存旧版本」几乎零成本，
/// 但不影响相等语义。
///
/// **一条 `imbl` 的实测事实**：元素少到能塞进 `Vector` 自己那几个指针的空间时
/// （inline 形态），克隆是逐值拷贝，堆上根本没有东西可比，`ptr_eq` 为假——所以这里
/// 用一份够大的历史。快路省的本来也是大历史的深比较，小历史深比较不要钱。
#[test]
fn messages_use_the_structural_sharing_fast_path_without_changing_the_answer() {
    let mut a: Vector<Message> = Vector::new();
    for i in 1..=8 {
        a.push_back(msg(i));
    }
    let b = a.clone();
    assert!(a.ptr_eq(&b), "克隆一份堆上的历史应该走指针快路");
    assert_eq!(AgentValue::Messages(a.clone()), AgentValue::Messages(b));

    // inline 形态没有指针可比，但**答案不变**——快路是快路，不是语义。
    let mut small: Vector<Message> = Vector::new();
    small.push_back(msg(1));
    assert_eq!(
        AgentValue::Messages(small.clone()),
        AgentValue::Messages(small.clone())
    );

    let mut c = a.clone();
    c.push_back(msg(9));
    assert!(!a.ptr_eq(&c));
    assert_ne!(AgentValue::Messages(a), AgentValue::Messages(c));
    assert_ne!(
        AgentValue::Messages(Vector::new()),
        AgentValue::Messages(small)
    );
}

/// 跨变体一律不等，含两个无载荷变体互相之间——`Null`（没有值）和 `Pending`
/// （还在等）是两件事，混同会让「工具还没回来」被当成「工具回了个空」。
#[test]
fn different_variants_are_never_equal() {
    let all = [
        AgentValue::Null,
        AgentValue::Pending,
        AgentValue::Bool(false),
        AgentValue::U64(0),
        AgentValue::Text(Arc::from("")),
        AgentValue::Json(Arc::new(serde_json::Value::Null)),
        AgentValue::Messages(Vector::new()),
        AgentValue::Status(TurnStatus::Idle),
        AgentValue::Prefix(PrefixImage { segments: Vec::new(), prompt_tokens: None }),
        AgentValue::Slots(Arc::new(Vec::new())),
    ];
    assert_eq!(all.len(), 10, "变体集合是封闭的：十个，一个不多");
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            assert_eq!(a == b, i == j, "{a:?} vs {b:?}");
        }
    }
}

/// 红线 3：每个变体都能 serde 往返。有一个不能，快照就是残的，而且是等到
/// 第一次真的从崩溃恢复时才发现。
#[test]
fn every_variant_survives_a_serde_roundtrip() {
    let mut history: Vector<Message> = Vector::new();
    history.push_back(msg(1));
    let all = vec![
        AgentValue::Null,
        AgentValue::Pending,
        AgentValue::Bool(true),
        AgentValue::U64(u64::MAX),
        AgentValue::Text(Arc::from("文本")),
        AgentValue::Json(Arc::new(serde_json::json!({"path": "/tmp/a"}))),
        AgentValue::Messages(history),
        AgentValue::Status(TurnStatus::Done { truncated: true }),
        AgentValue::Prefix(PrefixImage { segments: Vec::new(), prompt_tokens: Some(42) }),
        AgentValue::Slots(slots(2)),
    ];
    let s = serde_json::to_string(&all).unwrap();
    assert_eq!(serde_json::from_str::<Vec<AgentValue>>(&s).unwrap(), all);
}

#[test]
fn null_is_the_atom_value_null() {
    use agent_store::AtomValue;
    assert_eq!(AgentValue::null(), AgentValue::Null);
}

/// 取值器：对上返回内容，对不上返回 `None`（读取点据此 `expect`）。
#[test]
fn accessors_answer_none_on_a_variant_mismatch() {
    assert_eq!(AgentValue::U64(7).as_u64(), Some(7));
    assert_eq!(AgentValue::Bool(true).as_u64(), None);
    assert_eq!(AgentValue::Bool(true).as_bool(), Some(true));
    assert_eq!(AgentValue::Text(Arc::from("x")).as_text().map(|t| &**t), Some("x"));
    assert!(AgentValue::Json(Arc::new(serde_json::json!(1))).as_json().is_some());
    assert!(AgentValue::Messages(Vector::new()).as_messages().is_some());
    assert_eq!(AgentValue::Status(TurnStatus::Idle).as_status(), Some(&TurnStatus::Idle));
    assert!(AgentValue::Slots(Arc::new(Vec::new())).as_slots().is_some());
    assert!(AgentValue::Null.as_prefix().is_none());
    assert!(AgentValue::Pending.is_pending());
    assert!(!AgentValue::Null.is_pending());
}
