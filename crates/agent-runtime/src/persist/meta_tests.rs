//! [`PersistedMeta`] 的往返与**老会话文件迁移**。
//!
//! 拆成独立文件（`#[cfg(test)] #[path]`，同 `agent-core` 的 `restore_tests.rs`
//! 先例）：迁移这一组要用**真的老格式字节**，不能手搓结构体——手搓 `RawMeta` 只能
//! 证明 `From` 写对了，证明不了「老文件里那些字节真的还读得进来」，而后者才是
//! 199 §九 要的东西。

use super::*;

#[test]
fn a_known_label_round_trips_through_the_persisted_form() {
    let meta = EntryMeta {
        turn_id: 3,
        epoch: Epoch(7),
        label: "tool_result",
        undoability: Undoability::Blocked,
    };
    let persisted = PersistedMeta::from(&meta);
    assert_eq!(
        persisted,
        PersistedMeta {
            turn_id: 3,
            epoch: 7,
            label: "tool_result".to_string(),
            undoability: Undoability::Blocked,
        }
    );

    let back = EntryMeta::try_from(persisted).unwrap();
    assert_eq!(back, meta);
}

#[test]
fn an_unrecognized_label_is_rejected_not_guessed() {
    let persisted = PersistedMeta {
        turn_id: 1,
        epoch: 0,
        label: "some_future_label".to_string(),
        undoability: Undoability::StateOnly,
    };
    let err = EntryMeta::try_from(persisted).unwrap_err();
    assert_eq!(err, UnknownLabel("some_future_label".to_string()));
}

/// 199 之前的**真实落盘字节**（`crates/agent-runtime/src/persist/jsonl` 写出来的
/// 一行 meta 就长这样）：只有 `barrier: bool`，没有 `undoability`。
const V198_BARRIER_TRUE: &str = r#"{"turn_id":4,"epoch":2,"label":"tool_result","barrier":true}"#;
const V198_BARRIER_FALSE: &str = r#"{"turn_id":1,"epoch":0,"label":"user_input","barrier":false}"#;

/// 199 §九 的逐字确定映射：`barrier: true → Blocked`。
///
/// **这一条红了就是「老会话里一次真实的不可逆操作从此不再挡 undo」**——功能全部
/// 正常，只有某一天用户撤销时副作用悄悄留在了外面。
#[test]
fn an_old_session_file_with_barrier_true_loads_as_blocked() {
    let meta: PersistedMeta = serde_json::from_str(V198_BARRIER_TRUE).unwrap();
    assert_eq!(meta.undoability, Undoability::Blocked);
    assert_eq!(
        (meta.turn_id, meta.epoch, meta.label.as_str()),
        (4, 2, "tool_result")
    );

    // 翻回 core 的形状也得是屏障——`undo_turn` 读的就是这一位。整条恢复链
    // （老文件 → `recover` → `undo_turn` 真的被挡住）在
    // `tests/it/legacy_barrier_migration.rs` 上钉。
    let back = EntryMeta::try_from(meta).unwrap();
    assert_eq!(back.undoability, Undoability::Blocked);
}

/// 反向：`barrier: false → StateOnly`，**不是** `Hooked`。老会话本来就没有钩子，
/// 读成 `Hooked` 会让每一条老 entry 在 undo 时都去问一次不存在的钩子表，
/// 于是整条老会话变成不可撤销。
#[test]
fn an_old_session_file_with_barrier_false_loads_as_state_only() {
    let meta: PersistedMeta = serde_json::from_str(V198_BARRIER_FALSE).unwrap();
    assert_eq!(meta.undoability, Undoability::StateOnly);
}

/// 新格式写出去的字节里**没有** `barrier` 这个键了，而且读得回来（两个方向都验，
/// 免得只有 `From<RawMeta>` 对、`Serialize` 那半悄悄写成了别的形状）。
#[test]
fn the_new_form_serializes_undoability_and_reads_itself_back() {
    let meta = PersistedMeta {
        turn_id: 9,
        epoch: 3,
        label: "spawn_child".to_string(),
        undoability: Undoability::Hooked,
    };
    let line = serde_json::to_string(&meta).unwrap();
    assert_eq!(
        line,
        r#"{"turn_id":9,"epoch":3,"label":"spawn_child","undoability":"hooked"}"#
    );
    assert_eq!(serde_json::from_str::<PersistedMeta>(&line).unwrap(), meta);
}
