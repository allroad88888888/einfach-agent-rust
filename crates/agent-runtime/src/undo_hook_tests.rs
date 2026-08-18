//! 钩子表本身的单元测试（红线 9：从 `undo_hook.rs` 挪出来，源文件只留实现）。
//! `#[path]` 子模块，`super` 仍是 `undo_hook`，私有字段/方法照样够得着。
//!
//! 「一次真的 undo 把文件删掉」那条端到端验收不在这里——它要真跑一轮泵，住
//! `tests/it/ext_undo_fn_delivery.rs`。这个文件只钉表本身的四条规矩：
//! 查不到 = `Lost`、跑过一次之后再问答什么、`settle` 什么时候拒绝挂表、
//! 日志 cap 挤掉的钩子会不会被清。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::{
    AgentId, ContentBlock, DEFAULT_HISTORY_CAP, Event, HookOutcome, PrefixImage, Session,
    StopReason, TokenUsage, ToolCallId, Undoability,
};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use super::*;
use crate::tool_table::ToolTable;

fn ctx() -> RunnerCtx {
    let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "key".to_string(),
        fs,
        ToolTable::builtin(),
        Vec::new(),
        agent_core::SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

/// 一次真实的「模型点了一个工具 → 宿主登记 `mark_hooked` → 结果落地」序列，跟
/// `session_tool_ext::adapt` 在真机上做的是同一串事，只是这里手工喂事件。
/// 返回的会话最后一条 entry 就是那次调用的结果，档位 `Hooked`。
fn session_after_a_hooked_call(call_id: &str) -> (Session, ToolCallId) {
    let mut session = Session::new(AgentId::root());
    let _ = session.step(Event::UserInput {
        agent: AgentId::root(),
        text: "干件事".into(),
    });
    let call_id = ToolCallId::new(call_id);
    let _ = session.step(Event::ProviderDone {
        agent: AgentId::root(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::ToolUse {
            id: call_id.clone(),
            name: Arc::from("ext:demo/act"),
            input: Arc::new(serde_json::json!({})),
        }],
        stop: StopReason::ToolUse,
        usage: TokenUsage {
            prompt: 10,
            completion: 5,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
    session.mark_hooked(call_id.clone());
    let _ = session.step(Event::ToolResult {
        agent: AgentId::root(),
        epoch: session.epoch(),
        call_id: call_id.clone(),
        content: Arc::from("done"),
    });
    assert_eq!(
        session.last_entry().unwrap().meta.undoability,
        Undoability::Hooked,
        "夹具本身要先立住：`mark_hooked` 之后落地的那条 entry 该是 Hooked"
    );
    (session, call_id)
}

/// 一个记数的还原函数。`FnOnce` 不能直接数调用次数，所以数进一个共享计数器。
fn counting_undo(hits: Arc<AtomicUsize>, outcome: Result<(), &'static str>) -> UndoFn {
    Box::new(move || {
        hits.fetch_add(1, Ordering::SeqCst);
        outcome.map_err(Arc::from)
    })
}

/// 暂存 → 落地：函数挂到**刚落地那条 entry 的 `seq`** 上，undo 问它就跑得到。
#[test]
fn a_staged_function_lands_on_the_seq_of_the_entry_that_just_committed() {
    let mut ctx = ctx();
    let (session, call_id) = session_after_a_hooked_call("call_1");
    let seq = session.last_entry().unwrap().seq;
    let hits = Arc::new(AtomicUsize::new(0));

    ctx.undo_hooks
        .stage(call_id.clone(), counting_undo(Arc::clone(&hits), Ok(())));
    // 泵在 `step` 之前记下的那份 landing：结果落地之前日志末端是 tool_use 那条。
    settle(
        &mut ctx,
        &session,
        Some(Landing {
            call_id,
            seq_before: Some(seq - 1),
        }),
    );

    assert_eq!(ctx.undo_hooks.len(), 1, "该挂上一条");
    assert!(matches!(ctx.undo_hooks.run(seq), HookOutcome::Ok));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "钩子该真的被跑了一次");
}

/// **这次结果没能进日志**（epoch 闸挡掉 / 协议违规）时，函数丢掉，不硬挂到上一条
/// `Hooked` 的 entry 上——那条 entry 是**别人**的，跑它的逆就是撤了一件没发生的事。
#[test]
fn a_result_that_never_became_an_entry_drops_its_function_instead_of_hanging_it_elsewhere() {
    let mut ctx = ctx();
    let (session, _) = session_after_a_hooked_call("call_1");
    let seq = session.last_entry().unwrap().seq;
    let hits = Arc::new(AtomicUsize::new(0));

    // 第二次调用：函数暂存了，但 step 一条 entry 都没落（`seq_before` == 现在的末端）。
    let second = ToolCallId::new("call_2");
    ctx.undo_hooks
        .stage(second.clone(), counting_undo(Arc::clone(&hits), Ok(())));
    settle(
        &mut ctx,
        &session,
        Some(Landing {
            call_id: second,
            seq_before: Some(seq),
        }),
    );

    assert_eq!(
        ctx.undo_hooks.len(),
        0,
        "没有 entry 代表这次调用，表里就不该多出一条"
    );
    assert!(
        ctx.undo_hooks.nothing_staged(),
        "暂存区也该清干净，不留一个等不到 seq 的函数"
    );
}

/// 取消：队列里的事件不会再被 step，暂存区整片丢掉。
#[test]
fn cancelling_the_turn_discards_functions_that_will_never_land() {
    let mut ctx = ctx();
    let hits = Arc::new(AtomicUsize::new(0));
    ctx.undo_hooks.stage(
        ToolCallId::new("call_1"),
        counting_undo(Arc::clone(&hits), Ok(())),
    );
    assert!(!ctx.undo_hooks.nothing_staged());

    ctx.undo_hooks.discard_staged();

    assert!(ctx.undo_hooks.nothing_staged());
    assert_eq!(hits.load(Ordering::SeqCst), 0, "丢掉 ≠ 跑一遍");
}

/// 表里查不到 = **说好有函数、函数没了** = 进程重启过。答 `Lost` 而不是 `Ok`：
/// 答 `Ok` 就是静默跳过一次真实副作用（199 §九 点名的那个静默错值）。
#[test]
fn a_seq_the_table_never_heard_of_is_lost_not_ok() {
    let mut hooks = UndoHooks::default();
    assert!(matches!(hooks.run(7), HookOutcome::Lost));
}

/// 跑成功过的钩子再被问一次答 `Ok`，**不是 `Lost`**。
///
/// 这条路真实存在：undo（函数跑掉、文件没了）→ redo（状态回来了，**副作用不重放**，
/// 200 §5）→ 再 undo。此刻外部世界本来就已经干净了，只要退状态。
#[test]
fn a_hook_that_already_succeeded_answers_ok_again_and_does_not_run_twice() {
    let mut hooks = UndoHooks::default();
    let hits = Arc::new(AtomicUsize::new(0));
    hooks
        .table
        .insert(3, Hook::Ready(counting_undo(Arc::clone(&hits), Ok(()))));

    assert!(matches!(hooks.run(3), HookOutcome::Ok));
    assert!(matches!(hooks.run(3), HookOutcome::Ok));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "`FnOnce`：只跑一次");
}

/// 跑挂过的钩子再被问一次，**答同一句原因**——不会变成「函数随进程重启消失了」
/// （那是 `Lost` 的措辞，对一个刚在本进程里跑挂的钩子是假话）。
#[test]
fn a_hook_that_failed_repeats_the_same_reason_instead_of_claiming_it_vanished() {
    let mut hooks = UndoHooks::default();
    let hits = Arc::new(AtomicUsize::new(0));
    hooks.table.insert(
        3,
        Hook::Ready(counting_undo(Arc::clone(&hits), Err("磁盘只读"))),
    );

    let HookOutcome::Failed(first) = hooks.run(3) else {
        panic!("该失败");
    };
    let HookOutcome::Failed(again) = hooks.run(3) else {
        panic!("再问一次也该失败，而且是同一句");
    };
    assert_eq!(&*first, "磁盘只读");
    assert_eq!(first, again);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "`FnOnce`：只跑一次");
}

/// 201 验收第 4 条：**表随 history cap 清理**。
///
/// 造 `DEFAULT_HISTORY_CAP + 10` 条 entry（cap 把最老的 10 条挤掉了），给每条都挂
/// 一个钩子，然后清一次——挂在已经被挤掉那些 entry 上的钩子从此没有任何调用路径，
/// 留着就是只涨不落。
#[test]
fn hooks_of_entries_the_cap_dropped_are_cleaned_out() {
    let mut session = Session::new(AgentId::root());
    let total = DEFAULT_HISTORY_CAP + 10;
    for i in 0..total {
        // 每次写一个不同的值，保证真落一条 entry（值没变 `record_set` 不记）。
        session.set_max_turns(i as u32 + 1);
    }
    assert_eq!(session.history().entries().count(), DEFAULT_HISTORY_CAP);

    // 每条 entry 都挂一个（`seq` 由 `History` 铸造，从 0 起严格递增）。
    let mut hooks = UndoHooks::default();
    for seq in 0..total as u64 {
        hooks.table.insert(seq, Hook::Spent(None));
    }
    assert_eq!(hooks.len(), total);

    hooks.prune(&session);

    assert_eq!(
        hooks.len(),
        DEFAULT_HISTORY_CAP,
        "被 cap 挤掉的那 10 条 entry，钩子也该跟着走"
    );
    let oldest = session.history().entries().next().unwrap().seq;
    assert!(
        hooks.table.keys().all(|seq| *seq >= oldest),
        "留下的键必须都还在日志里：{:?}",
        hooks.table.keys().collect::<Vec<_>>()
    );
}
