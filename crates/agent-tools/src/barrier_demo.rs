//! 020「范围裁决」第 2 条：屏障机制的 `History` 级演示。
//!
//! `srv:shell/exec` 是第一个 `Reversibility::Irreversible` 的工具，017 已经在
//! `agent-store` 里把「undo 撞上 `barrier(meta)` 为真的条目就停下、推
//! `Blocked`」焊死并测过（`agent-store/tests/undo_redo_barrier.rs`）。这个文件
//! **不重新发明那套机制**，只证明一件事：本 issue 判给 `srv:shell/exec` 的
//! `Irreversible` 接得上那套已经存在的屏障——机制端到端可用，`agent-tools`
//! 这一侧缺的只是把它接进真实的 command log/CLI（那是集成 issue 的事，
//! `docs/issues/020-shell-tool.md`「范围裁决」）。
//!
//! **刻意不用 `agent_store::Store`**：真正接进原子图时，「把 `Change.prev`/
//! `next` 写回状态」必须走 agent-core 的 command 层（红线 2），而那层在
//! `agent-tools`（一个既不是 `agent-store` 也不是 `agent-core::command` 的
//! crate）里还不存在——本 issue 的范围也明确没有它。所以这里的「应用」只是
//! 把 `applied` 条目里的 `prev`/`next` 赋给一个本地变量，单纯观察游标和
//! `UndoOutcome` 本身的行为；真正的落库路径留给「状态搬进原子图」集成 issue。
//!
//! 只是 `#[cfg(test)]` 演示，不是 `agent-tools` 的公开 API；`History` 来自
//! dev-dependency（见 `Cargo.toml` 注释）。

use agent_store::{Change, History, UndoOutcome};

/// 日志 meta。真实集成时这里装的是 agent-core 的 `Reversibility`——`History<K,
/// V, M>` 的 `M` 就是留给上层塞这类词汇的占位（agent-store 不认识
/// `Reversibility` 这个词，见 `agent-store/src/history/log.rs`）。这里只留
/// `irreversible: bool`，对应 `Reversibility::Irreversible`。
#[derive(Clone, Copy, Debug, PartialEq)]
struct ToolCallMeta {
    tool: &'static str,
    irreversible: bool,
}

/// 镜像 `agent-runtime::tool_table::reversibility_of` 的判断（那个 crate 依赖
/// 这个 crate，方向不能反过来，所以这里不 import，照抄同一条规则）：已知的
/// 纯读工具是 `Pure`，其余——包括本 issue 新增的 `srv:shell/exec`——保守落
/// `Irreversible`。
fn is_known_pure(tool: &str) -> bool {
    matches!(tool, "srv:fs/read" | "srv:fs/list")
}

fn call(tool: &'static str) -> ToolCallMeta {
    ToolCallMeta { tool, irreversible: !is_known_pure(tool) }
}

/// undo 的屏障谓词：`Reversibility::Irreversible` ⇒ 挡。
fn is_barrier(meta: &ToolCallMeta) -> bool {
    meta.irreversible
}

fn same_turn(a: &ToolCallMeta, b: &ToolCallMeta) -> bool {
    // 演示只关心屏障，不关心 turn 边界——把整条日志当一个 turn。
    let _ = (a, b);
    true
}

type Log = History<String, i64, ToolCallMeta>;

/// 记一步：`prev` 取调用方手上「当前值」的快照，`next` 是这次工具调用的效果。
/// 不经过任何 `Store`——`current` 就是一个普通的本地变量，模拟「这个工具调用
/// 之后世界变成了什么样」。
fn write(history: &mut Log, current: &mut i64, next: i64, meta: ToolCallMeta) -> u64 {
    let change = Change { key: "workspace".to_string(), prev: *current, next };
    *current = next;
    history.append(meta, vec![change]).unwrap()
}

/// 把 `UndoOutcome::{Applied,Blocked}` 里 `applied` 的条目按 undo 语义（倒序、
/// 写 `prev`）回放进 `current`——纯本地赋值，不是店铺式写入。
fn rollback(current: &mut i64, applied: &[agent_store::Entry<String, i64, ToolCallMeta>]) {
    for entry in applied {
        for change in entry.changes.iter().rev() {
            *current = change.prev;
        }
    }
}

/// 验收核心：undo 越过一次 `srv:shell/exec` 时停下并报 `Blocked`，不静默回滚。
#[test]
fn undo_stops_at_the_door_of_a_shell_exec_entry() {
    let mut history: Log = History::new();
    let mut current = 0i64;

    // 先一次纯读（不是屏障），再一次 shell/exec（是屏障，紧邻游标）。
    write(&mut history, &mut current, 1, call("srv:fs/read"));
    let barrier_seq = write(&mut history, &mut current, 2, call("srv:shell/exec"));

    let before_cursor = history.cursor();
    match history.undo_one(is_barrier) {
        UndoOutcome::Blocked { applied, barrier_seq: bs } => {
            assert!(applied.is_empty(), "门口即屏障，没有任何条目该被应用");
            assert_eq!(bs, barrier_seq);
        }
        other => panic!("expected Blocked，got {other:?}"),
    }
    assert_eq!(history.cursor(), before_cursor, "撞门口屏障，游标不动");
    assert_eq!(current, 2, "shell/exec 已经跑过的效果原样保留——不是静默回滚");
}

/// `undo_turn`（UI 默认粒度）中途撞上 `shell/exec`：屏障之后的纯读被弹出，
/// 屏障本身连同它之前的一切原样保留，游标恰好停在屏障后一格。
#[test]
fn undo_turn_stops_one_slot_past_a_mid_turn_shell_exec() {
    let mut history: Log = History::new();
    let mut current = 0i64;

    write(&mut history, &mut current, 1, call("srv:fs/read")); // e0：纯读
    let barrier_seq = write(&mut history, &mut current, 2, call("srv:shell/exec")); // e1：屏障
    write(&mut history, &mut current, 3, call("srv:fs/list")); // e2：纯读
    write(&mut history, &mut current, 4, call("srv:fs/read")); // e3：纯读

    let applied = match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked { applied, barrier_seq: bs } => {
            assert_eq!(bs, barrier_seq);
            applied
        }
        other => panic!("expected Blocked，got {other:?}"),
    };
    assert_eq!(applied.len(), 2, "屏障之后的 e3、e2 被弹出，屏障本身（e1）不弹");
    rollback(&mut current, &applied);
    assert_eq!(current, 2, "停在 shell/exec 生效之后的状态，不是它生效之前");
}

/// redo 不受屏障约束：从屏障刚生效的位置往前，能直接把它之后的纯读 redo 回来。
#[test]
fn redo_crosses_the_shell_exec_barrier_freely() {
    let mut history: Log = History::new();
    let mut current = 0i64;

    write(&mut history, &mut current, 1, call("srv:fs/read")); // e0
    write(&mut history, &mut current, 2, call("srv:shell/exec")); // e1，屏障
    write(&mut history, &mut current, 3, call("srv:fs/read")); // e2

    let applied = match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked { applied, .. } => applied,
        other => panic!("expected Blocked，got {other:?}"),
    };
    rollback(&mut current, &applied);
    assert_eq!(current, 2);

    let redone = match history.redo_turn(same_turn) {
        UndoOutcome::Applied(entries) => entries,
        other => panic!("expected Applied（redo 不看屏障），got {other:?}"),
    };
    for entry in &redone {
        for change in entry.changes.iter() {
            current = change.next;
        }
    }
    assert_eq!(current, 3, "屏障不挡 redo");
}
