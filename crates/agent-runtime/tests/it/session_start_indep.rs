//! 独立测试：只依据 issue 135（docs/issues/135-session-start-driver.md）「验收」/
//! 「注意」两节 + `docs/INVARIANTS.md` 红线 11 + 公开签名
//! `agent_runtime::{ToolTable, CallTiming, run_session_start, SessionStartError}`
//! 写成，**不看** `crates/agent-runtime/src/session_start.rs` /
//! `subagent.rs` / `runner.rs` 里的实现体。实现由另一个 agent 并行写，本文件与它
//! 互不通信。
//!
//! 本文件只管 `run_session_start` **自己的契约**——顺序 / 空文本 / 失败 / 恢复
//! 不重跑，四条都不碰 provider/wire。前缀块怎么真的进请求体字节是另一件事，见
//! `session_start_prompt_indep.rs`（红线 11 那一半）。
//!
//! # 假定的被测契约（写在委派任务里，未见实现体）
//!
//! ```ignore
//! pub struct SessionStartError { pub tool: Arc<str>, pub message: Arc<str> }
//! pub fn run_session_start(session: &mut Session, tools: &ToolTable) -> Result<(), SessionStartError>;
//! ```
//!
//! 语义：按注册顺序执行 `timed(SessionStart)` 的每个工具；`Ok` 非空文本 →
//! `SystemChunk { label: "init:<name>", text }`；空文本跳过；任一 `Err` → 整体
//! `Err` 且一个前缀块都不写（history 无 `prefix_init` entry）；全部成功 → 一次
//! `set_prefix_chunks`。
//!
//! 四条测试对应四条验收（逐条见各测试函数上的文档注释）：
//! 1. 顺序 = 注册顺序，交换注册顺序 → 顺序跟着换。
//! 2. 空文本的工具不产块，且不打断前后两块的相对顺序。
//! 3. 失败工具 → 整体 `Err`（携带该工具名）、无前缀块、history 无 `prefix_init`。
//! 4. 执行计数：新建 = 1 次；快照 → 恢复 → 不再调 `run_session_start` → 仍 1 次。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::{AgentId, Session, SystemChunk, ToolSpec, DEFAULT_HISTORY_CAP};
use agent_runtime::{run_session_start, CallTiming, TimedRun, ToolTable};
use serde_json::json;

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

fn chunk(label: &str, text: &str) -> SystemChunk {
    SystemChunk {
        label: Arc::from(label),
        text: Arc::from(text),
    }
}

/// 总是成功、回一段固定文本的执行体。
fn ok_text(text: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable, _input: &serde_json::Value| -> Result<Arc<str>, Arc<str>> {
            Ok(Arc::from(text))
        },
    )
}

/// 总是成功、但文本是空串——验收第 2 条要覆盖的「不产块」路径。
fn ok_empty() -> TimedRun {
    ok_text("")
}

/// 总是失败，回一段固定错误消息。
fn err_text(message: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable, _input: &serde_json::Value| -> Result<Arc<str>, Arc<str>> {
            Err(Arc::from(message))
        },
    )
}

/// 成功执行体，每次真的被调都往 `counter` 上加一——验收第 4 条数「到底跑了几次」。
fn counting_ok(counter: Arc<AtomicUsize>, text: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable, _input: &serde_json::Value| -> Result<Arc<str>, Arc<str>> {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::from(text))
        },
    )
}

/// 验收 1 上半：两个 fake `SessionStart` 工具 → `prefix_chunks()` 恰两块，
/// label 分别是 `"init:<name>"`，顺序 = 注册顺序。
#[test]
fn two_session_start_tools_produce_two_chunks_in_registration_order() {
    let table = ToolTable::empty()
        .with_timed(
            spec("alpha", "第一个开局工具"),
            CallTiming::SessionStart,
            ok_text("来自 alpha 的问候"),
        )
        .with_timed(
            spec("beta", "第二个开局工具"),
            CallTiming::SessionStart,
            ok_text("来自 beta 的问候"),
        );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("两个工具都该成功");

    assert_eq!(
        session.prefix_chunks(),
        vec![
            chunk("init:alpha", "来自 alpha 的问候"),
            chunk("init:beta", "来自 beta 的问候"),
        ],
        "顺序该是注册顺序，label 该是 init:<name>"
    );
}

/// 验收 1 下半：交换注册顺序 → `prefix_chunks()` 的顺序跟着换。
#[test]
fn swapping_registration_order_swaps_the_prefix_chunk_order() {
    let table = ToolTable::empty()
        .with_timed(
            spec("beta", "第二个开局工具"),
            CallTiming::SessionStart,
            ok_text("来自 beta 的问候"),
        )
        .with_timed(
            spec("alpha", "第一个开局工具"),
            CallTiming::SessionStart,
            ok_text("来自 alpha 的问候"),
        );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("两个工具都该成功");

    assert_eq!(
        session.prefix_chunks(),
        vec![
            chunk("init:beta", "来自 beta 的问候"),
            chunk("init:alpha", "来自 alpha 的问候"),
        ],
        "交换注册顺序之后，前缀块的顺序必须跟着换——不是按名字排序之类的稳定序"
    );
}

/// 验收 2：返回空文本的工具不产块，且不打断前后两块（alpha/beta）的相对顺序。
#[test]
fn a_tool_returning_empty_text_produces_no_chunk_and_does_not_break_ordering() {
    let table = ToolTable::empty()
        .with_timed(
            spec("alpha", "有内容"),
            CallTiming::SessionStart,
            ok_text("A 的内容"),
        )
        .with_timed(
            spec("silent", "空文本，不该产块"),
            CallTiming::SessionStart,
            ok_empty(),
        )
        .with_timed(
            spec("beta", "有内容"),
            CallTiming::SessionStart,
            ok_text("B 的内容"),
        );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("三个工具都返回 Ok，只是 silent 是空文本");

    assert_eq!(
        session.prefix_chunks(),
        vec![chunk("init:alpha", "A 的内容"), chunk("init:beta", "B 的内容")],
        "空文本的工具不产块，且前后两块仍按注册顺序相邻"
    );
}

/// 验收 3：fake 失败工具（排第二）→ 整体 `Err`（携带该工具名）；
/// `prefix_chunks()` 为空、history 里没有 `"prefix_init"` entry——全有或全无，
/// 不留半份前缀。
#[test]
fn a_failing_tool_aborts_the_whole_batch_and_leaves_no_prefix_trace() {
    let table = ToolTable::empty()
        .with_timed(
            spec("alpha", "会成功"),
            CallTiming::SessionStart,
            ok_text("A 的内容"),
        )
        .with_timed(
            spec("boom", "会失败"),
            CallTiming::SessionStart,
            err_text("装不上"),
        )
        .with_timed(
            spec("gamma", "排在失败工具之后"),
            CallTiming::SessionStart,
            ok_text("C 的内容"),
        );

    let mut session = Session::new(AgentId::root());
    let before = session.history_len();
    let err = run_session_start(&mut session, &table).expect_err("第二个工具失败，整体该是 Err");

    assert_eq!(&*err.tool, "boom", "错误必须携带失败的那个工具名");
    assert_eq!(
        &*err.message, "装不上",
        "错误消息该是执行体返回的 Err 原文"
    );

    assert!(
        session.prefix_chunks().is_empty(),
        "任一失败 → 一个前缀块都不写"
    );
    assert_eq!(
        session.history_len(),
        before,
        "失败路径不该留下任何新 entry——全有或全无"
    );
    assert!(
        session
            .history()
            .entries()
            .all(|e| e.meta.label != "prefix_init"),
        "history 里不该出现 prefix_init entry"
    );
}

/// 验收 4：执行计数器 = 1（新建）；对 session 做快照 → `Session::restore` 出新
/// `Session`（**不再调** `run_session_start`——这是调用方契约，134 落的状态就是
/// 恢复路径的唯一数据来源）→ `prefix_chunks()` 逐字节相同，计数器仍然是 1
/// （驱动没有在恢复路径上被偷跑）。
#[test]
fn restoring_from_a_snapshot_does_not_rerun_session_start_and_keeps_prefix_chunks_identical() {
    let calls = Arc::new(AtomicUsize::new(0));
    let table = ToolTable::empty().with_timed(
        spec("alpha", "唯一一个开局工具"),
        CallTiming::SessionStart,
        counting_ok(Arc::clone(&calls), "只该被算一次的问候"),
    );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("唯一的工具该成功");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "新建会话该执行一次");

    let original_chunks = session.prefix_chunks();
    assert_eq!(
        original_chunks,
        vec![chunk("init:alpha", "只该被算一次的问候")]
    );

    // 快照 → 恢复一个新 Session。调用方契约：恢复路径**不再调** run_session_start，
    // 前缀块的值直接从 134 落的状态（`Slot::PrefixChunks`）读回来。
    let snapshot = session.primitives();
    let restored = Session::restore(
        AgentId::root(),
        Some(snapshot),
        Vec::new(),
        0,
        0,
        DEFAULT_HISTORY_CAP,
        &mut |key| panic!("恢复不该遇到不认识的键：{key:?}"),
    )
    .expect("从刚拿到的快照恢复不该失败");

    assert_eq!(
        restored.prefix_chunks(),
        original_chunks,
        "恢复出的前缀块必须与快照前逐字节相同"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "恢复路径不该偷跑执行体——计数器必须仍然是 1，不是 2"
    );
}
