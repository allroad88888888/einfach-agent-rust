//! 独立测试：只依据 issue 133（docs/issues/133-call-timing-field.md）「验收」/
//! 「注意」两节 + `agent_runtime::{ToolTable, CallTiming, TimedTool, TimedRun}`
//! 的公开签名写成，**不看** `crates/agent-runtime/src/tool_table*.rs` 里的实现体。
//! 实现由另一个 agent 并行写，本文件与它互不通信。
//!
//! # 假定的公开签名（写在委派任务里的契约，未见实现体）
//!
//! ```ignore
//! pub enum CallTiming { SessionStart, TurnEnd }  // Clone/Copy/PartialEq/Eq/Debug
//! pub type TimedRun =
//!     Box<dyn Fn(&ToolTable, &Session, &serde_json::Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>;
//! impl ToolTable {
//!     pub fn with_timed(self, spec: ToolSpec, timing: CallTiming, run: TimedRun) -> Self;
//!     pub fn timed(&self, timing: CallTiming) -> impl Iterator<Item = &TimedTool>;
//! }
//! pub struct TimedTool { .. }
//! impl TimedTool {
//!     pub fn spec(&self) -> &ToolSpec;
//!     pub fn run(&self, table: &ToolTable, session: &Session, input: &serde_json::Value) -> Result<Arc<str>, Arc<str>>;
//! }
//! ```
//!
//! 签名注（153，决策 30）：`TimedRun`/`TimedTool::run` 加了只读 `&Session`——
//! 本文件写成时（133）还没有这个参数，独立测试作为公开类型演进的机械跟随只补
//! 参数列表，不改任何断言。
//!
//! 六条断言对应六条验收（逐条见各测试函数上的文档注释）：
//! 1. timed 工具不出现在 `specs()`；`declares()` 为假。
//! 2. `timed(SessionStart)` 迭代顺序 == 注册顺序，交换注册顺序迭代顺序跟着换。
//! 3. `SessionStart` 与 `TurnEnd` 两个区互不串台。
//! 4. 执行体真的被调、能读到 input、`Ok`/`Err` 两路都测。
//! 5. `ToolTable::builtin()` 的 timed 区为空，且它的 `specs()` 序列化与
//!    「`builtin()` 再 `with_timed` 一个工具」的 `specs()` 序列化逐字节相同
//!    （红线 11：timed 注册不碰模型面的表）。
//! 6. 撞名：与 specs 区已有名字同名的 `with_timed` 在 debug 构建下 panic。
//!
//! 第 6 条用 `"srv:fs/read"` 当撞名靶子——这个名字确认存在于
//! `ToolTable::builtin()` 里（`happy_two_hop.rs` 经 `support::build_ctx`
//! 用同一张表真执行过它，`tool_executor_seam_needs_no_filesystem.rs` 同款），
//! 不是本文件凭空猜的名字。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_core::{AgentId, Session, ToolSpec};
use agent_runtime::{CallTiming, TimedRun, TimedTool, ToolTable};
use serde_json::json;

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 什么都不做、总是 `Ok` 的执行体——用在只关心注册/查询行为、不关心执行结果
/// 的测试里，每次调用现造一个新的 `Box`（`TimedRun` 不是 `Clone`）。
fn ok_run() -> TimedRun {
    Box::new(
        |_table: &ToolTable,
         _session: &Session,
         _input: &serde_json::Value|
         -> Result<Arc<str>, Arc<str>> { Ok(Arc::from("ok")) },
    )
}

/// 验收 1：timed 工具不出现在 `specs()`；`declares()` 为假——模型看不见它，
/// 也调不到它（issue 133 §目标）。
#[test]
fn timed_tools_are_absent_from_specs_and_declares() {
    let table = ToolTable::empty().with_timed(
        spec("srv:timing/on-start", "会话开局跑一次的时机工具"),
        CallTiming::SessionStart,
        ok_run(),
    );

    assert!(
        table.specs().is_empty(),
        "timed 工具不该出现在喂模型的 specs 里，实际: {:?}",
        table.specs()
    );
    assert!(
        !table.declares("srv:timing/on-start"),
        "declares() 必须为假——它是模型面清单的唯一判据"
    );
}

/// 验收 2：`timed(SessionStart)` 迭代顺序 == 注册顺序；交换两个工具的注册顺序，
/// 迭代顺序跟着换（跟 `specs()` 的 push 顺序保证同一种承诺）。
#[test]
fn timed_session_start_iterates_in_registration_order_and_follows_a_swap() {
    let ab = ToolTable::empty()
        .with_timed(spec("srv:timing/a", "第一个"), CallTiming::SessionStart, ok_run())
        .with_timed(spec("srv:timing/b", "第二个"), CallTiming::SessionStart, ok_run());
    let names_ab: Vec<&str> = ab
        .timed(CallTiming::SessionStart)
        .map(|t| t.spec().name.as_ref())
        .collect();
    assert_eq!(names_ab, vec!["srv:timing/a", "srv:timing/b"]);

    // 交换注册顺序：先 b 后 a。
    let ba = ToolTable::empty()
        .with_timed(spec("srv:timing/b", "第二个"), CallTiming::SessionStart, ok_run())
        .with_timed(spec("srv:timing/a", "第一个"), CallTiming::SessionStart, ok_run());
    let names_ba: Vec<&str> = ba
        .timed(CallTiming::SessionStart)
        .map(|t| t.spec().name.as_ref())
        .collect();
    assert_eq!(
        names_ba,
        vec!["srv:timing/b", "srv:timing/a"],
        "交换注册顺序之后，迭代顺序必须跟着换——不是按名字排序之类的稳定序"
    );
}

/// 验收 3：`SessionStart` 与 `TurnEnd` 是两个独立区，互不串台——注册进一个
/// 时机的工具不会出现在另一个时机的迭代里。
#[test]
fn session_start_and_turn_end_do_not_cross_contaminate() {
    let table = ToolTable::empty()
        .with_timed(spec("srv:timing/start", "开局跑"), CallTiming::SessionStart, ok_run())
        .with_timed(spec("srv:timing/end", "收尾跑"), CallTiming::TurnEnd, ok_run());

    let start_names: Vec<&str> = table
        .timed(CallTiming::SessionStart)
        .map(|t| t.spec().name.as_ref())
        .collect();
    let end_names: Vec<&str> = table
        .timed(CallTiming::TurnEnd)
        .map(|t| t.spec().name.as_ref())
        .collect();

    assert_eq!(
        start_names,
        vec!["srv:timing/start"],
        "SessionStart 区不该混进 TurnEnd 注册的工具"
    );
    assert_eq!(
        end_names,
        vec!["srv:timing/end"],
        "TurnEnd 区不该混进 SessionStart 注册的工具"
    );
}

/// 验收 4：执行体真的被调（闭包捕获 `Arc<AtomicUsize>` 计数）、能读到调用方
/// 传入的 `input`，`Ok`/`Err` 两路都测。
#[test]
fn run_is_actually_invoked_reads_input_and_covers_both_ok_and_err_paths() {
    let ok_calls = Arc::new(AtomicUsize::new(0));
    let seen_input: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));

    let ok_run: TimedRun = {
        let ok_calls = Arc::clone(&ok_calls);
        let seen_input = Arc::clone(&seen_input);
        Box::new(
            move |_table: &ToolTable,
                  _session: &Session,
                  input: &serde_json::Value|
                  -> Result<Arc<str>, Arc<str>> {
                ok_calls.fetch_add(1, Ordering::SeqCst);
                *seen_input.lock().unwrap() = Some(input.clone());
                Ok(Arc::from("done"))
            },
        )
    };
    let err_run: TimedRun = Box::new(
        |_table: &ToolTable,
         _session: &Session,
         _input: &serde_json::Value|
         -> Result<Arc<str>, Arc<str>> { Err(Arc::from("boom")) },
    );

    let table = ToolTable::empty()
        .with_timed(spec("srv:timing/ok", "总是成功"), CallTiming::SessionStart, ok_run)
        .with_timed(spec("srv:timing/err", "总是失败"), CallTiming::SessionStart, err_run);

    let entries: Vec<&TimedTool> = table.timed(CallTiming::SessionStart).collect();
    let ok_entry = entries
        .iter()
        .find(|t| t.spec().name.as_ref() == "srv:timing/ok")
        .expect("srv:timing/ok 该在 SessionStart 区里");
    let err_entry = entries
        .iter()
        .find(|t| t.spec().name.as_ref() == "srv:timing/err")
        .expect("srv:timing/err 该在 SessionStart 区里");

    let session = Session::new(AgentId::root());
    let input = json!({"probe": "value-x"});
    let result = ok_entry.run(&table, &session, &input);
    assert_eq!(result, Ok(Arc::from("done")));
    assert_eq!(
        ok_calls.load(Ordering::SeqCst),
        1,
        "run() 必须真的调用了注册时给的闭包，而不是只返回一个占位结果"
    );
    assert_eq!(
        seen_input.lock().unwrap().as_ref(),
        Some(&input),
        "闭包必须能读到调用方传入的 input，不是一份空的或者别的输入"
    );

    let err_result = err_entry.run(&table, &session, &json!({}));
    assert_eq!(err_result, Err(Arc::from("boom")), "Err 路径必须原样透传");
    // 再调一次 ok_entry，确认计数是累加的而不是「第一次调用之后失效」。
    let _ = ok_entry.run(&table, &session, &json!({"probe": "again"}));
    assert_eq!(ok_calls.load(Ordering::SeqCst), 2);
}

/// 验收 5 上半：`ToolTable::builtin()` 的 timed 区为空——builtin() 只是既有的
/// 模型自主调工具表，没有任何工具被预先注册进 timed 区。
#[test]
fn builtin_has_no_timed_tools() {
    let base = ToolTable::builtin();
    assert_eq!(
        base.timed(CallTiming::SessionStart).count(),
        0,
        "builtin() 不该预置任何 SessionStart 时机工具"
    );
    assert_eq!(
        base.timed(CallTiming::TurnEnd).count(),
        0,
        "builtin() 不该预置任何 TurnEnd 时机工具"
    );
}

/// 验收 5 下半（红线 11 看门狗）：`builtin()` 的 `specs()` 序列化，与「`builtin()`
/// 再 `with_timed` 一个工具」之后的 `specs()` 序列化**逐字节相同**——timed 注册
/// 完全不碰模型面的那张表，哪怕一个字节。只比较 `Vec` 长度或者工具名集合不够，
/// 一个「顺手把 timed 条目也序列化进某个字段」的实现会被这条挡住。
#[test]
fn adding_a_timed_tool_to_builtin_leaves_specs_bytes_byte_identical() {
    let base = ToolTable::builtin();
    let base_bytes = serde_json::to_vec(base.specs()).expect("specs() 必须能序列化");

    let with_one = ToolTable::builtin().with_timed(
        spec("srv:timing/extra", "额外注册的时机工具，不该影响模型面的表"),
        CallTiming::SessionStart,
        ok_run(),
    );
    let with_one_bytes = serde_json::to_vec(with_one.specs()).expect("specs() 必须能序列化");

    assert_eq!(
        base_bytes, with_one_bytes,
        "红线 11：with_timed 之后 specs() 的字节必须与之前逐字节相同"
    );
}

/// 验收 6：timed 名与 specs 区已有名字相撞——debug 构建下必须 panic
/// （`debug_assert!`，069 判据的延续：一个名字只能有一条执行路径）。
/// `#[cfg(debug_assertions)]` 门住整条测试——release 构建里 `debug_assert!` 是
/// 空操作，测这条会误红。
#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn with_timed_panics_in_debug_when_the_name_collides_with_an_existing_spec() {
    // "srv:fs/read" 确认存在于 ToolTable::builtin() 里（见文件头注释）。
    let _ = ToolTable::builtin().with_timed(
        spec("srv:fs/read", "撞名的时机工具——这个名字已经在 builtin() 的 specs 里"),
        CallTiming::SessionStart,
        ok_run(),
    );
}
