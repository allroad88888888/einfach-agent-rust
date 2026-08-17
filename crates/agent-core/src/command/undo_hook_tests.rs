//! [`super`] 的白盒单测：**钩子跑在回滚之前**、失败怎么停、`/undo!` 怎么只越过
//! 一条、逆序。拆成独立文件（`#[cfg(test)] #[path]`，同 `spawn_tests.rs` /
//! `restore_tests.rs` 的先例）。
//!
//! 夹具走 [`Session::restore`]（真实的恢复路径）而不是喂事件：这一组要精确控制
//! **每一条 entry 是哪一档、写的是哪个 atom、prev/next 各是多少**，才能断言钩子在
//! 失败的那一刻读到的是回滚前还是回滚后的值。喂事件造得出 `Hooked`（那条路在
//! `tests/it/session_undo_hook.rs` 上钉），但造不出「同一个 atom 在同一轮里被三条
//! entry 依次 +1」这种一眼看得出先后的值链。

use std::sync::Arc;

use agent_store::Change;

use crate::engine::epoch::Epoch;
use crate::graph::{AtomKey, Slot, source_atom};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

use super::super::meta::{AgentEntry, EntryMeta, Undoability};
use super::super::spawn::AgentLimits;
use super::super::undo::UndoReport;
use super::*;

/// 被这些 entry 反复写的那一个槽位。挑 `TurnsUsed` 是因为它持 `U64`——值链
/// `0→1→2→3` 一眼看得出「退到哪一步了」。
fn key() -> AtomKey {
    AtomKey::Agent(AgentId::root(), Slot::TurnsUsed)
}

/// 第 `seq` 条：把槽位从 `seq` 写成 `seq + 1`，档位由调用方给。全部同一个 turn，
/// 于是一次 `undo_turn` 就是整条链。
fn entry(seq: u64, undoability: Undoability) -> AgentEntry {
    AgentEntry {
        seq,
        meta: EntryMeta {
            turn_id: 1,
            epoch: Epoch(0),
            label: "tool_result",
            undoability,
        },
        changes: vec![Change {
            key: key(),
            prev: AgentValue::U64(seq),
            next: AgentValue::U64(seq + 1),
        }],
    }
}

fn session_of(undoabilities: &[Undoability]) -> Session {
    let entries: Vec<AgentEntry> = undoabilities
        .iter()
        .enumerate()
        .map(|(i, u)| entry(i as u64, *u))
        .collect();
    let cursor = entries.len();
    let next_seq = cursor as u64;
    Session::restore(
        AgentId::root(),
        None,
        entries,
        cursor,
        next_seq,
        100,
        AgentLimits::default(),
        &mut |_| panic!("这些键都是本版认识的"),
    )
    .expect("夹具自洽")
}

/// 当前槽位的值。白盒读（`Session` 刻意不暴露 store）；钩子里读同一个值要自己
/// 捏一份 store 句柄的克隆——那时 `Session` 正被可变借用，钩子拿不到 `&Session`。
fn read(session: &Session) -> u64 {
    let atom = source_atom(&session.store, &session.sources, &key());
    session.store.get(atom).as_u64().expect("TurnsUsed 持 U64")
}

fn failed(why: &str) -> HookOutcome {
    HookOutcome::Failed(Arc::from(why))
}

/// 两个报告构造器：断言写成一行，读起来就是「退了几条、停在哪条、为什么」。
fn blocked(entries: usize, barrier_seq: u64, cause: BlockedCause) -> UndoReport {
    UndoReport::Blocked {
        entries,
        barrier_seq,
        cause,
    }
}

fn applied(entries: usize) -> UndoReport {
    UndoReport::Applied {
        entries,
        turn_id: 1,
    }
}

fn hook_failed(why: &str) -> BlockedCause {
    BlockedCause::HookFailed(Arc::from(why))
}

// ——— 1. 顺序：钩子必须看见回滚**之前**的值 ——————————————————————

/// **本 issue 最硬的一条。** 三条 entry（`StateOnly` / `Hooked` / `StateOnly`），
/// 钩子在 seq=1 上失败，失败时读一次 store。
///
/// 值链是 `0→1→2→3`，此刻槽位是 3。逆序走：seq=2 先退（3→2），轮到 seq=1 时
/// 它自己还**没**退，所以钩子该读到 **2**。
///
/// 三种写错各有各的读数，所以一条断言把它们全区分开：正确 = **2**；「先
/// `apply_prev` 一整批再回头跑钩子」= 0；「这一条先退再跑它的钩子」= 1。头一种
/// 正是红线导言点名的静默错值：store 说这一步没发生过，外部世界里它还在。
#[test]
fn the_hook_runs_before_this_entry_is_rolled_back() {
    let mut session = session_of(&[
        Undoability::StateOnly,
        Undoability::Hooked,
        Undoability::StateOnly,
    ]);
    assert_eq!(read(&session), 3);

    let store = session.store.clone();
    let atom = source_atom(&session.store, &session.sources, &key());
    let mut seen = None;
    let report = {
        let mut hook = |e: &AgentEntry| {
            seen = Some(store.get(atom).as_u64().unwrap());
            assert_eq!(e.seq, 1, "只有 Hooked 那一条该被问到");
            failed("CRM 返回 409")
        };
        session.undo_turn_with(&mut hook)
    };

    assert_eq!(
        seen,
        Some(2),
        "钩子必须在这一条被回滚**之前**跑——读到 0 说明整批先退了，读到 1 说明这一条先退了"
    );
    assert_eq!(report, blocked(1, 1, hook_failed("CRM 返回 409")));
    // 比它新的那条已经退掉；失败的这一条**停在新值上**（199 §五）。
    assert_eq!(read(&session), 2);
    assert_eq!(session.cursor(), 2, "游标要停在失败那一条的后面");
}

/// 停下之后 `redo_turn` 仍然能把已经退掉的那一条追回来——游标没被推错格
/// （`recede_cursor` 写漏一格的话，这里会多退/少退一条）。
#[test]
fn the_cursor_after_a_failed_hook_is_still_consistent_with_the_store() {
    let mut session = session_of(&[
        Undoability::StateOnly,
        Undoability::Hooked,
        Undoability::StateOnly,
    ]);
    let _ = session.undo_turn_with(&mut |_| failed("挂了"));
    assert_eq!((read(&session), session.cursor()), (2, 2));

    assert_eq!(session.redo_turn(), applied(1));
    assert_eq!((read(&session), session.cursor()), (3, 3));
}

// ——— 2. 钩子随进程重启没了 ——————————————————————————————

/// `Hooked` 但钩子表里查不到（恢复之后的常态：函数是闭包，不跨进程）→
/// `HookLost`，**不是**静默跳过。静默跳过 = 状态回滚了、外部世界里那次副作用
/// 还在，而且没有任何人被告知（199 §九 点名的那条）。
#[test]
fn a_hooked_entry_whose_hook_is_gone_blocks_with_hook_lost() {
    let mut session = session_of(&[Undoability::Hooked, Undoability::StateOnly]);
    let report = session.undo_turn_with(&mut |_| HookOutcome::Lost);
    assert_eq!(report, blocked(1, 0, BlockedCause::HookLost));
    assert_eq!(read(&session), 1);
}

// ——— 3. `/undo!`：一次确认只放行一条 ————————————————————————

/// 同一轮里两条钩子都会失败：`undo_turn_force` 越过**第一条**之后接着退，撞上
/// 第二条再停一次、再问一次。放行全部等于让一次确认替用户答了几个他没被问到的
/// 问题（027 定、199 §五 复核）。
#[test]
fn force_crosses_one_failing_hook_at_a_time() {
    let mut session = session_of(&[
        Undoability::StateOnly,
        Undoability::Hooked,
        Undoability::Hooked,
        Undoability::StateOnly,
    ]);
    // 非 force：撞上 seq=2 就停（seq=3 已经退掉）。
    let report = session.undo_turn_with(&mut |_| failed("挂了"));
    assert_eq!(report, blocked(1, 2, hook_failed("挂了")));
    assert_eq!(read(&session), 3);

    // 第一次 force：越过 seq=2（状态照退），撞上 seq=1 再停。
    let report = session.undo_turn_force_with(&mut |_| failed("挂了"));
    assert_eq!(report, blocked(1, 1, hook_failed("挂了")));
    assert_eq!(read(&session), 2);

    // 第二次 force：越过 seq=1，剩下的 seq=0 是干净的，一路退到底。
    let report = session.undo_turn_force_with(&mut |_| failed("挂了"));
    assert_eq!(report, applied(2));
    assert_eq!((read(&session), session.cursor()), (0, 0));
}

/// 混合形态：一条钩子失败（新）+ 一条屏障（老）。额度该花在**先遇到的那个**。
///
/// 这一条钉的是一个会**活锁**的写法：`History` 那侧的谓词只看得见 `&EntryMeta`，
/// 会乐观地放过更老的那条屏障；额度要是就此算花掉，钩子失败这条永远越不过去
/// ——用户按多少次 `/undo!` 都停在同一个地方。
#[test]
fn force_spends_its_one_crossing_on_the_obstacle_it_meets_first() {
    let mut session = session_of(&[
        Undoability::Blocked,
        Undoability::StateOnly,
        Undoability::Hooked,
    ]);
    let report = session.undo_turn_force_with(&mut |_| failed("挂了"));
    // 越过 seq=2（钩子失败），seq=1 干净地退掉，停在 seq=0 那条屏障上。
    assert_eq!(report, blocked(2, 0, BlockedCause::NoHook));
    assert_eq!((read(&session), session.cursor()), (1, 1));

    // 再确认一次才轮到屏障。
    let report = session.undo_turn_force_with(&mut |_| failed("挂了"));
    assert_eq!(report, applied(1));
    assert_eq!((read(&session), session.cursor()), (0, 0));
}

// ——— 4. 逆序 ————————————————————————————————————————

/// 钩子被调用的顺序是 `seq` **降序**。逆序是论文 Theorem 16 里唯一不需要前提的
/// 顺序（任意顺序要求 effects 两两独立，而那是我们无法验证的性质）——所以它是
/// 要求不是巧合，这条测试就是那条要求的钉子。
#[test]
fn hooks_run_newest_first() {
    let mut session = session_of(&[Undoability::Hooked; 3]);
    let mut order = Vec::new();
    let report = {
        let mut hook = |e: &AgentEntry| {
            order.push(e.seq);
            HookOutcome::Ok
        };
        session.undo_turn_with(&mut hook)
    };
    assert_eq!(order, vec![2, 1, 0]);
    assert_eq!(report, applied(3));
    assert_eq!(read(&session), 0);
}

/// 只有 `Hooked` 会被问。这条同时是「无参 `undo_turn()` 与 199 之前逐字节相同」
/// 的举证：老会话里一条 `Hooked` 都没有（迁移只映射出 `Blocked`/`StateOnly`），
/// 那个恒 `Ok` 的钩子一次都不会被调到。
#[test]
fn state_only_entries_are_never_asked() {
    let mut session = session_of(&[Undoability::StateOnly; 3]);
    let mut asked = 0;
    let report = {
        let mut hook = |_: &AgentEntry| {
            asked += 1;
            HookOutcome::Ok
        };
        session.undo_turn_with(&mut hook)
    };
    assert_eq!(asked, 0, "没碰过外部世界的 entry 不该去问钩子表");
    assert_eq!(report, applied(3));
}

/// 屏障（`Blocked`）走的仍然是 `History` 那道谓词：钩子一次都不问，成因是
/// `NoHook`，屏障那条一个字节都没动。
#[test]
fn a_barrier_still_blocks_without_asking_any_hook() {
    let mut session = session_of(&[Undoability::Blocked, Undoability::StateOnly]);
    let mut asked = 0;
    let report = {
        let mut hook = |_: &AgentEntry| {
            asked += 1;
            HookOutcome::Ok
        };
        session.undo_turn_with(&mut hook)
    };
    assert_eq!(asked, 0);
    assert_eq!(report, blocked(1, 0, BlockedCause::NoHook));
    assert_eq!(read(&session), 1);
}

/// `undo_step`（开发者档）走的是同一条逐条循环。
#[test]
fn undo_step_runs_the_hook_too() {
    let mut session = session_of(&[Undoability::StateOnly, Undoability::Hooked]);
    let report = session.undo_step_with(&mut |_| failed("挂了"));
    assert_eq!(report, blocked(0, 1, hook_failed("挂了")));
    // 一条都没退：失败那条就在游标下。
    assert_eq!((read(&session), session.cursor()), (2, 2));
}
