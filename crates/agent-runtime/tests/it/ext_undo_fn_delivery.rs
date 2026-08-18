//! 201 的**主验收**：一个扩展工具在真实文件系统上干了一件事、把「怎么撤回去」的
//! 函数交回来，`/undo` 真的把它撤掉（决策 199 §一 §三）。
//!
//! 这里没有 mock：**真的 `RunnerCtx` + 真的泵（`run_turn`）+ 真的落盘的文件**。
//! 假的只有模型那一端（脚本化 SSE，跟同目录其余集成测试同一套 `support`）——
//! 被验的是「工具交回的函数有没有在正确的时刻被正确地调用」，不是模型会不会点它。
//!
//! # 四条，对应 issue 201 验收第一条的四步
//!
//! 1. [`an_extension_tool_that_hands_back_an_undo_fn_gets_its_file_removed_by_undo]：
//!    调它 → 文件在；`/undo` → **文件真的没了**，`UndoReport::Applied`。
//! 2. [`a_failing_undo_fn_blocks_the_undo_and_leaves_both_worlds_untouched`]：
//!    还原失败 → `Blocked { cause: HookFailed }`，**文件还在**，而且那条 entry 的
//!    **状态没回滚**（tool_result 还在消息里）。这是 199 §三 那张表的第一行：
//!    还原没成，store 就不该往回走——两个世界一致地停在原地。
//! 3. 同一条测试的后半：`/undo!` 越过它继续退，**文件仍然在**（用户已确认接受），
//!    而且还原函数**没有被跑第二次**（`FnOnce` / 论文 §5.1.1 的 `armed`）。
//! 4. [`a_tool_that_touched_nothing_never_stops_an_undo`]：交 `Aftermath::Nothing`
//!    的工具一路放行——「没碰外部世界」和「碰了但撤不回」不是一回事，这是 199
//!    全部的要点。
//!
//! # 怎么让「删除失败」是**真的**失败
//!
//! issue 原文的做法是「把文件设成只读」。Unix 上删一个文件看的是**父目录**的写权限
//! 而不是文件自己的，所以那条路要 chmod 目录，而 root 又能绕过权限位——在容器里跑
//! 的 CI 会静默地变成「删成功了」，这条测试就白测了。
//!
//! 这里改用一个**任何身份都会失败**的真实系统调用：还原函数要删的是工具建出来的
//! 那个工作目录，而目录里还躺着**别人**放的一个文件 → `ENOTEMPTY`。失败是真的
//! （`std::fs::remove_dir` 的返回值），文件也确实还在，跟 issue 想验的事逐字一致。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::{
    AgentId, BlockedCause, ContentBlock, Session, ToolSpec, TurnStatus, UndoReport, Undoability,
};
use agent_providers::wire_name;
use agent_runtime::{Aftermath, ExtensionPack, SessionToolFn, ToolTable, run_turn};
use serde_json::{Value, json};

use crate::support;

const TOUCH_TOOL: &str = "ext:undo/touch";
const READ_TOOL: &str = "ext:undo/peek";
const SENTINEL: &str = "EXT-UNDO-FN-SENTINEL-3b7e";

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 被测的那条扩展工具：在真实文件系统上**建一个文件**，并把「删掉它」交回来。
///
/// `undo` 是这条工具的全部要害——它捕获的是**执行那一刻**的现场（这次建出来的
/// 那条路径），这正是 199 §一 说的「逆是在执行的那个状态上选的」。它**没有捕获
/// `&Session`**：还原跑在 undo 路上，那时 core 正在回滚状态，让它同时写状态就是在
/// 一次回滚中间插一次前向写入。
fn touch_tool(dir: PathBuf, undo_kind: UndoKind, ran: Arc<AtomicUsize>) -> SessionToolFn {
    Box::new(
        move |_session: &mut Session, _agent: &AgentId, _input: &Value| {
            let workspace = dir.join("made-by-ext");
            std::fs::create_dir_all(&workspace).map_err(|e| Arc::from(e.to_string().as_str()))?;
            let note = workspace.join("note.txt");
            std::fs::write(&note, SENTINEL).map_err(|e| Arc::from(e.to_string().as_str()))?;

            let ran = Arc::clone(&ran);
            let target = match undo_kind {
                UndoKind::RemoveTheFile => note.clone(),
                UndoKind::RemoveTheWorkspaceDir => workspace.clone(),
            };
            let undo: agent_runtime::UndoFn = Box::new(move || {
                ran.fetch_add(1, Ordering::SeqCst);
                let outcome = match undo_kind {
                    UndoKind::RemoveTheFile => std::fs::remove_file(&target),
                    // 目录里还有别人的文件 → `ENOTEMPTY`，任何身份都删不掉。
                    UndoKind::RemoveTheWorkspaceDir => std::fs::remove_dir(&target),
                };
                outcome
                    .map_err(|e| Arc::from(format!("收拾不掉 {}：{e}", target.display()).as_str()))
            });
            Ok((
                Arc::from(format!("wrote {}", note.display())),
                Aftermath::Undo(undo),
            ))
        },
    )
}

#[derive(Clone, Copy)]
enum UndoKind {
    /// 撤得掉的那种。
    RemoveTheFile,
    /// 撤不掉的那种（目录非空）。
    RemoveTheWorkspaceDir,
}

/// 一条什么都不碰的截获工具，用来钉「`Nothing` 不是屏障」那一条。
fn peek_tool() -> SessionToolFn {
    Box::new(|_session: &mut Session, _agent: &AgentId, _input: &Value| {
        Ok((Arc::from(SENTINEL), Aftermath::Nothing))
    })
}

/// 跑一轮：模型点一次这个扩展工具，然后收尾。返回会话与 ctx，供各条测试接着撤销。
fn run_one_turn(
    dir: &std::path::Path,
    tool: &str,
    pack: ExtensionPack,
) -> (Session, agent_runtime::RunnerCtx) {
    let wire = wire_name::to_wire(tool);
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_1", &wire, "{}"),
        support::sse_text("done"),
    ]);
    let (tools, pending) = ToolTable::builtin().with_extension(pack);
    let (mut ctx, _events) = support::build_ctx_with(port, dir, tools);
    pending.install(&mut ctx);

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call the ext tool")
        .expect("scripted turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    (session, ctx)
}

/// 这个 agent 的历史里还找得到 `call_id` 那次调用的结果吗——「那条 entry 的状态
/// 有没有被回滚」的判据：tool_result 那条 entry 干的事就是把结果写进消息历史。
fn has_tool_result(session: &Session, call_id: &str) -> bool {
    session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .any(|block| matches!(block, ContentBlock::ToolResult { id, .. } if &*id.0 == call_id))
}

/// 验收第 1、2 步：调它 → 文件在；`/undo` → 文件**真的没了**。
#[test]
fn an_extension_tool_that_hands_back_an_undo_fn_gets_its_file_removed_by_undo() {
    let dir = support::temp_dir("ext-undo-fn-happy");
    let ran = Arc::new(AtomicUsize::new(0));
    let pack = ExtensionPack::new("undo").with_tool(
        spec(TOUCH_TOOL, "建一个文件，并交回删掉它的函数"),
        touch_tool(dir.clone(), UndoKind::RemoveTheFile, Arc::clone(&ran)),
    );
    let (mut session, mut ctx) = run_one_turn(&dir, TOUCH_TOOL, pack);

    let note = dir.join("made-by-ext").join("note.txt");
    assert!(note.exists(), "工具真的写了文件：{}", note.display());
    assert_eq!(ran.load(Ordering::SeqCst), 0, "还没撤销，还原函数不该跑");
    assert_eq!(
        session.last_entry().unwrap().meta.undoability,
        Undoability::StateOnly,
        "最后一条是收尾那次 provider_done，它没碰外部世界"
    );

    let report = agent_runtime::undo::undo_turn(&mut session, &mut ctx);

    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "交了还原函数就该撤得掉（不是屏障）：{report:?}"
    );
    assert!(
        !note.exists(),
        "**文件该没了**——这条一红就说明还原函数根本没被调用：{}",
        note.display()
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1, "还原函数恰好跑一次");
    assert!(session.messages().is_empty(), "这一轮该整个退掉");
}

/// 验收第 3、4 步：还原失败 → `Blocked { HookFailed }`，**文件还在**、那条 entry 的
/// **状态没回滚**；`/undo!` 才越过，越过之后文件仍然在，函数也没被跑第二次。
#[test]
fn a_failing_undo_fn_blocks_the_undo_and_leaves_both_worlds_untouched() {
    let dir = support::temp_dir("ext-undo-fn-failing");
    let ran = Arc::new(AtomicUsize::new(0));
    let pack = ExtensionPack::new("undo").with_tool(
        spec(TOUCH_TOOL, "建一个文件，交回一个注定失败的还原函数"),
        touch_tool(
            dir.clone(),
            UndoKind::RemoveTheWorkspaceDir,
            Arc::clone(&ran),
        ),
    );
    let (mut session, mut ctx) = run_one_turn(&dir, TOUCH_TOOL, pack);

    let workspace = dir.join("made-by-ext");
    let note = workspace.join("note.txt");
    assert!(note.exists());
    // 别人往那个目录里放了一个文件——于是 `remove_dir` 必然 `ENOTEMPTY`（见文件头
    // 「怎么让删除失败是真的失败」）。
    std::fs::write(workspace.join("intruder.txt"), "not mine").unwrap();

    let report = agent_runtime::undo::undo_turn(&mut session, &mut ctx);

    let UndoReport::Blocked {
        barrier_seq, cause, ..
    } = report.clone()
    else {
        panic!("还原失败该停下来问，拿到 {report:?}");
    };
    let BlockedCause::HookFailed(why) = &cause else {
        panic!("成因该是 HookFailed（碰了、可能做了一半），拿到 {cause:?}");
    };
    assert!(
        why.contains("收拾不掉"),
        "原因该是还原函数自己那句话，好让用户判断要不要强制越过：{why}"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1, "还原函数跑过一次（失败了）");

    // 外部世界：文件还在。
    assert!(note.exists(), "还原失败了，文件当然还该在");
    // 状态：**那条 entry 没回滚**（199 §三 表第一行：还原没成，store 就不该往回走）。
    assert!(
        has_tool_result(&session, "call_1"),
        "卡住那条 entry 的状态不该被回滚——两个世界要一致地停在原地"
    );
    let blocked_entry = session
        .history()
        .entries()
        .find(|e| e.seq == barrier_seq)
        .expect("停在的那条该还在日志里");
    assert_eq!(
        blocked_entry.meta.undoability,
        Undoability::Hooked,
        "停在的是那条**交过还原函数**的 entry，不是别的屏障"
    );

    // `/undo!`：用户确认「继续，副作用不回滚」。
    let report = agent_runtime::undo::undo_turn_force(&mut session, &mut ctx);

    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "强制越过该成功：{report:?}"
    );
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
    assert!(
        note.exists(),
        "越过 = 跳过这一步的**还原**，文件仍然在（用户已经确认接受这件事）"
    );
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "`FnOnce`：越过那一下不该把已经跑挂的还原函数再跑一遍"
    );
}

/// 交 `Aftermath::Nothing` 的工具一路放行——「没碰外部世界」不是「碰了但撤不回」。
#[test]
fn a_tool_that_touched_nothing_never_stops_an_undo() {
    let dir = support::temp_dir("ext-undo-fn-nothing");
    let pack = ExtensionPack::new("undo")
        .with_tool(spec(READ_TOOL, "什么都不碰，只回一句话"), peek_tool());
    let (mut session, mut ctx) = run_one_turn(&dir, READ_TOOL, pack);

    let entry = session
        .history()
        .entries()
        .find(|e| e.meta.label == "tool_result")
        .expect("这一轮该有一条 tool_result entry");
    assert_eq!(
        entry.meta.undoability,
        Undoability::StateOnly,
        "什么都没交 = StateOnly，既不是钩子也不是屏障"
    );

    let report = agent_runtime::undo::undo_turn(&mut session, &mut ctx);
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "纯读不该挡住 undo：{report:?}"
    );
}
