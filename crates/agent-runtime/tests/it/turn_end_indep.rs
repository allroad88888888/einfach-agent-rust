//! 独立测试：只依据 issue 136（`docs/issues/136-turn-end-driver.md`）「验收」/
//! 「注意」两节 + 公开 API（`agent_runtime::{ToolTable, CallTiming, TimedRun,
//! run_turn, RunnerCtx}`、`agent_core::Session` 的既有公开面）写成，**不看**
//! `crates/agent-runtime/src/turn_end.rs`、`runner.rs` 等驱动实现体。实现由另一个
//! agent 并行写，本文件与它互不通信。
//!
//! 复杂文件豁免（>300 行、≤500 行）：六条测试是同一个驱动契约的六个角度，
//! 拆开会把「6 条测试对应 6 条验收」这条映射打散，且落点被委派任务锁定为
//! 「唯二两个文件」（本文件 + `main.rs` 加一行），拆出共享 helper 模块不在允许
//! 改动范围内——跟同批次 133 独立测试 `call_timing_indep.rs` 同一个取舍。
//!
//! `CallTiming`/`ToolTable::with_timed`/`ToolTable::timed`/`TimedRun` 的签名取自
//! 133 已经落地并被其独立测试（`call_timing_indep.rs`）验证过的公开面
//! （`agent_runtime` 的 `pub use`），不是 136 本条驱动的实现——读它跟读
//! `agent_core::Session` 的公开方法签名是同一类允许，不是「看实现体」。
//!
//! # 被测契约（委派任务原文）
//!
//! 每个以 `TurnStatus::Done{..}` 正常完成的轮结束后，表里 `TurnEnd` 时机的工具按
//! 注册顺序被各调一次（入参 `Null`，结果丢弃）；取消/失败/非终态的轮不触发；hook
//! 返回 `Err` 不影响轮的结果、不 panic；hook 不落 store、不进 prompt。
//!
//! # 六条测试对应「要覆盖的验收」六条
//!
//! 1. [`turn_end_hook_fires_exactly_once_per_completed_turn`]：计数 hook，N 个
//!    完成轮 → 计数恰 N。
//! 2. [`two_turn_end_hooks_fire_in_registration_order`]：两个 hook 按注册顺序
//!    被调。
//! 3. [`cancelled_turn_does_not_fire_the_turn_end_hook`]：取消的轮 → 计数不增
//!    （照抄 `cancel.rs` 的取消模拟手法）。
//! 4. [`turn_end_hook_returning_err_does_not_change_the_turn_status_or_panic`]：
//!    hook 返回 `Err` → `TurnStatus` 与无 hook 时相同、不 panic。
//! 5. [`turn_end_hook_does_not_write_any_history_entry`]：不落 store——带 hook 与
//!    不带 hook 两个会话各跑一轮，`history_len()` 与每条 entry 的 label 序列逐条
//!    相同。
//! 6. [`turn_end_hook_name_and_output_never_appear_in_the_next_round_request_body`]：
//!    不进 prompt——选了「捕获真实请求体」这条路（`support::spawn_recording_server`
//!    已经现成），断言 hook 的名字与输出字节都不在第二轮请求体里；没有退到
//!    Ingredients/encode 字节比对那条备选路。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::{AgentId, Failure, Session, ToolSpec, TurnStatus};
use agent_runtime::{run_turn, CallTiming, TimedRun, ToolTable};
use serde_json::{json, Value};

use crate::support;
use crate::support::ScriptedResponse;

/// 一条 timed 工具的 spec 骨架——测试只关心名字（撞名判据）与它是否出现在模型面
/// 的表里，schema/description 内容本身不是断言对象。
fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 验收 1：注册一个计数 hook（闭包捕获 `Arc<AtomicUsize>`），跑 N 个完成轮 →
/// 计数恰好 N——每个 `Done` 轮之后 `TurnEnd` 工具被调一次，不多不少。
///
/// 第 2 轮起要显式 `session.begin_turn()`（跟 `ladder_ten_consecutive_
/// compactions.rs` 同一个坑：漏了不报错，新一轮的 `UserInput` 撞上上一轮的
/// `Done` 会被判成 `Notice::ProtocolViolation`，那一轮压根没发生任何请求，
/// 计数也就不会增加——为了不把「begin_turn 漏了」和「hook 没接上」这两种失败
/// 原因混在一起，这里显式处理游标推进）。
#[test]
fn turn_end_hook_fires_exactly_once_per_completed_turn() {
    const N: usize = 3;
    let dir = support::temp_dir("turn-end-count");
    let responses: Vec<ScriptedResponse> = (0..N)
        .map(|i| support::sse_text(&format!("第 {i} 轮回复")))
        .collect();
    let port = support::spawn_scripted_server(responses);

    let count = Arc::new(AtomicUsize::new(0));
    let hook_count = Arc::clone(&count);
    let run: TimedRun = Box::new(move |_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        hook_count.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::from("ok"))
    });
    let tools = ToolTable::builtin().with_timed(
        spec("srv:timing/turn-end-count", "每轮完成后计数一次"),
        CallTiming::TurnEnd,
        run,
    );

    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    for i in 0..N {
        if i > 0 {
            session.begin_turn();
        }
        let status = run_turn(&mut session, &mut ctx, &format!("第 {i} 句"))
            .unwrap_or_else(|e| panic!("第 {i} 轮不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "第 {i} 轮该正常完成");
    }

    assert_eq!(
        count.load(Ordering::SeqCst),
        N,
        "N 个完成轮之后计数必须恰好是 N，不多不少"
    );
}

/// 验收 2：两个 hook 按注册顺序被调——用一个共享 `Vec<&str>` 记调用序，注册顺序
/// 是 a 先 b 后，实际调用序必须跟着是 `["a", "b"]`。
#[test]
fn two_turn_end_hooks_fire_in_registration_order() {
    let dir = support::temp_dir("turn-end-order");
    let port = support::spawn_scripted_server(vec![support::sse_text("收到")]);

    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let order_a = Arc::clone(&order);
    let order_b = Arc::clone(&order);

    let run_a: TimedRun = Box::new(move |_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        order_a.lock().unwrap().push("a");
        Ok(Arc::from("a"))
    });
    let run_b: TimedRun = Box::new(move |_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        order_b.lock().unwrap().push("b");
        Ok(Arc::from("b"))
    });

    let tools = ToolTable::builtin()
        .with_timed(spec("srv:timing/turn-end-a", "第一个注册的 hook"), CallTiming::TurnEnd, run_a)
        .with_timed(spec("srv:timing/turn-end-b", "第二个注册的 hook"), CallTiming::TurnEnd, run_b);

    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "你好").expect("不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    assert_eq!(
        *order.lock().unwrap(),
        vec!["a", "b"],
        "两个 hook 必须按 with_timed 的注册顺序被调，不是任意顺序或反序"
    );
}

/// 验收 3：取消的轮不触发 hook——照抄 `cancel.rs`（验收清单第三条）的取消模拟
/// 手法：假服务器只写响应头就挂住不回，后台线程 200ms 后置位 `cancel_flag`，
/// 超时预算特意拉到 5s（远大于这条测试的时间尺度），确保观察到的终态是取消
/// 标志起的作用，不是我们自己的超时机制凑巧撞上。`run_turn` 该落
/// `Failed(Cancelled)`，计数必须停在 0——它既不是 `Done`，也不该被排除法之外的
/// 什么隐藏路径碰到 hook。
#[test]
fn cancelled_turn_does_not_fire_the_turn_end_hook() {
    let dir = support::temp_dir("turn-end-cancel");
    let port = support::spawn_scripted_server(vec![ScriptedResponse::HangAfterHeaders]);

    let count = Arc::new(AtomicUsize::new(0));
    let hook_count = Arc::clone(&count);
    let run: TimedRun = Box::new(move |_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        hook_count.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::from("ok"))
    });
    let tools = ToolTable::builtin().with_timed(
        spec("srv:timing/turn-end-cancel-count", "取消的轮不该调到这里"),
        CallTiming::TurnEnd,
        run,
    );

    let (ctx, _events) = support::build_ctx_with(port, &dir, tools);
    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(5));

    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    let mut session = Session::new(AgentId::root());
    let status =
        run_turn(&mut session, &mut ctx, "你好").expect("cancellation is not a source failure");

    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "取消的轮不是 Done，TurnEnd hook 不该被调到"
    );
}

/// 验收 4：hook 返回 `Err` 不影响轮的结果、不 panic——用一个无 hook 的基线会话
/// 对照，两者跑同一句话之后的 `TurnStatus` 必须相同。测试函数本身跑到底而不是
/// 中途 abort 就是「不 panic」的直接证据（`run_turn` 是同步调用，hook 的执行体
/// 在同一线程内跑完才返回）。
#[test]
fn turn_end_hook_returning_err_does_not_change_the_turn_status_or_panic() {
    let dir_baseline = support::temp_dir("turn-end-err-baseline");
    let dir_hook = support::temp_dir("turn-end-err-hook");

    let port_baseline = support::spawn_scripted_server(vec![support::sse_text("回复")]);
    let port_hook = support::spawn_scripted_server(vec![support::sse_text("回复")]);

    let (mut ctx_baseline, _events_baseline) = support::build_ctx(port_baseline, &dir_baseline);

    let run: TimedRun = Box::new(|_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        Err(Arc::from("boom"))
    });
    let tools = ToolTable::builtin().with_timed(
        spec("srv:timing/turn-end-err", "总是失败的 hook"),
        CallTiming::TurnEnd,
        run,
    );
    let (mut ctx_hook, _events_hook) = support::build_ctx_with(port_hook, &dir_hook, tools);

    let mut session_baseline = Session::new(AgentId::root());
    let mut session_hook = Session::new(AgentId::root());

    let status_baseline = run_turn(&mut session_baseline, &mut ctx_baseline, "你好")
        .expect("baseline 不该是 source failure");
    let status_hook = run_turn(&mut session_hook, &mut ctx_hook, "你好")
        .expect("hook 返回 Err 不该让 run_turn 本身失败，也不该 panic");

    assert_eq!(
        status_hook, status_baseline,
        "hook 返回 Err 之后 TurnStatus 必须与无 hook 时相同"
    );
    assert_eq!(status_hook, TurnStatus::Done { truncated: false });
}

/// 验收 5：hook 不落 store——带 hook 与不带 hook 两个会话各跑一轮同样的话，
/// `history_len()` 与每条 entry 的 label 序列（`Session::history().entries()`
/// 的 `meta.label`）必须逐条相同。先断言 hook 真的被调用了一次，否则下面的
/// 「相同」比较毫无意义（可能只是两边都没跑到 hook）。
#[test]
fn turn_end_hook_does_not_write_any_history_entry() {
    let dir_no_hook = support::temp_dir("turn-end-no-store-baseline");
    let dir_hook = support::temp_dir("turn-end-no-store-hook");

    let port_no_hook = support::spawn_scripted_server(vec![support::sse_text("回复")]);
    let port_hook = support::spawn_scripted_server(vec![support::sse_text("回复")]);

    let (mut ctx_no_hook, _events_no_hook) = support::build_ctx(port_no_hook, &dir_no_hook);

    let hook_calls = Arc::new(AtomicUsize::new(0));
    let hook_calls_run = Arc::clone(&hook_calls);
    let run: TimedRun = Box::new(move |_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        hook_calls_run.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::from("ok"))
    });
    let tools = ToolTable::builtin().with_timed(
        spec("srv:timing/turn-end-no-store", "验证不落 store 的 hook"),
        CallTiming::TurnEnd,
        run,
    );
    let (mut ctx_hook, _events_hook) = support::build_ctx_with(port_hook, &dir_hook, tools);

    let mut session_no_hook = Session::new(AgentId::root());
    let mut session_hook = Session::new(AgentId::root());

    run_turn(&mut session_no_hook, &mut ctx_no_hook, "同一句话")
        .expect("无 hook 会话不该是 source failure");
    run_turn(&mut session_hook, &mut ctx_hook, "同一句话")
        .expect("有 hook 会话不该是 source failure");

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        1,
        "hook 必须真的被调用了一次——否则下面的「相同」比较毫无意义"
    );

    assert_eq!(
        session_hook.history_len(),
        session_no_hook.history_len(),
        "有 hook 的会话不该比无 hook 的会话多任何一条 command log entry"
    );

    let labels_no_hook: Vec<&str> = session_no_hook
        .history()
        .entries()
        .map(|e| e.meta.label)
        .collect();
    let labels_hook: Vec<&str> = session_hook
        .history()
        .entries()
        .map(|e| e.meta.label)
        .collect();
    assert_eq!(
        labels_hook, labels_no_hook,
        "两个会话的 entry label 序列必须逐条相同——hook 的副作用不进 command log"
    );
}

/// 验收 6：hook 不进 prompt——带 hook 的会话跑第二轮，用
/// `support::spawn_recording_server` 捕获第二轮实际发出去的请求体，断言 hook 的
/// 名字与它的执行体输出字节都不在里面。
///
/// 选了「捕获真实请求体」这条路，不是退而求其次的 Ingredients/encode 字节比对：
/// `support` 模块已经有现成的 `spawn_recording_server`（`prefix_intent_*` 系列
/// 测试也在用），能拿到第二轮真正发出去的 JSON body，没有必要退一步。
#[test]
fn turn_end_hook_name_and_output_never_appear_in_the_next_round_request_body() {
    let dir = support::temp_dir("turn-end-no-prompt");

    let hook_name = "srv:timing/turn-end-no-prompt-marker-zzz9f3c";
    let hook_output = "TURN-END-HOOK-OUTPUT-MARKER-ZZZ9F3C";

    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_text("第一轮回复"),
        support::sse_text("第二轮回复"),
    ]);

    let hook_output_owned: Arc<str> = Arc::from(hook_output);
    let run: TimedRun = Box::new(move |_table: &ToolTable, _session: &Session, _input: &Value| -> Result<Arc<str>, Arc<str>> {
        Ok(Arc::clone(&hook_output_owned))
    });
    let tools = ToolTable::builtin().with_timed(spec(hook_name, "不该进 prompt 的 hook"), CallTiming::TurnEnd, run);

    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status1 =
        run_turn(&mut session, &mut ctx, "第一句话").expect("第一轮不该是 source failure");
    assert_eq!(status1, TurnStatus::Done { truncated: false });

    session.begin_turn();
    let status2 =
        run_turn(&mut session, &mut ctx, "第二句话").expect("第二轮不该是 source failure");
    assert_eq!(status2, TurnStatus::Done { truncated: false });

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "两轮各一次请求：{bodies:#?}");
    let second_round_body = &bodies[1];

    assert!(
        !second_round_body.contains(hook_name),
        "hook 的名字不该出现在第二轮请求体里：{second_round_body}"
    );
    assert!(
        !second_round_body.contains(hook_output),
        "hook 的输出字节不该出现在第二轮请求体里：{second_round_body}"
    );
}
