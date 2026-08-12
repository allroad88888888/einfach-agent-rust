//! 独立测试：`inherit_prefix` 落的状态在快照/恢复路径上的稳定性，以及看门狗
//! ——`SessionStart` 执行体的调用次数不因为子 agent 的出生而增加。只依据
//! `docs/issues/145-spawn-inherit-prefix.md`「验收」「注意」两节 + 决策 28 +
//! 公开 API 写成，**不看**任何实现体；模型面校验/过滤契约在姊妹文件
//! `inherit_prefix_indep.rs`（职责分开：那份管「参数怎么被校验/过滤」，这份管
//! 「这套机制在恢复路径和计数上稳不稳」）。
//!
//! 快照/恢复手法照抄 `session_start_indep.rs`「验收 4」：`session.primitives()`
//! → `Session::restore(...)`，恢复路径不重跑 `run_session_start`；本文件把
//! 同一手法搬到「子已经被 spawn 出来、带着 `PrefixAllowed`」这个更复杂的状态
//! 上，钉住 144/145 落的槽位跟着整棵树一起复原，不是只有 root 那一份。
//! 看门狗手法照抄 `session_start_indep.rs::counting_ok`。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::{AgentId, AgentLimits, DEFAULT_HISTORY_CAP, Session, ToolSpec, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{CallTiming, TimedRun, ToolTable, run_session_start, run_turn};
use serde_json::json;

use crate::support;

const MARKER: &str = "INHERIT-PREFIX-RESTORE-MARKER-77fe";

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 成功执行体，每次真的被调都往 `counter` 上加一——照
/// `session_start_indep.rs::counting_ok`，看门狗数「到底跑了几次」。
fn counting_ok(counter: Arc<AtomicUsize>, text: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable,
              _session: &Session,
              _input: &serde_json::Value|
              -> Result<Arc<str>, Arc<str>> {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::from(text))
        },
    )
}

/// 请求体里 `role: "system"` 那条消息的正文。手法照抄
/// `session_start_prompt_indep.rs`/`skill_switch_wire_indep.rs::wire_system_text`。
fn wire_system_text(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("请求体该是合法 JSON");
    let messages = value["messages"].as_array().expect("请求体里该有 messages");
    messages
        .iter()
        .find(|m| m["role"] == "system")
        .expect("该有一条 system 消息")["content"]
        .as_str()
        .expect("system 消息该有文本正文")
        .to_string()
}

fn spawn_wire() -> String {
    wire_name::to_wire("srv:agent/spawn")
}

/// `srv:agent/spawn` 的入参，编成 `support::sse_tool_call` 的 `arguments` 参数
/// 要的原文——照 `inherit_prefix_indep.rs::spawn_input` 同一个手法：JSON 对象
/// `to_string()` 之后再当一个 JSON 字符串整体转义一次。
fn spawn_input(value: serde_json::Value) -> String {
    let raw = value.to_string();
    let escaped = serde_json::to_string(&raw).expect("字符串序列化不该失败");
    escaped[1..escaped.len() - 1].to_string()
}

fn table(counter: Arc<AtomicUsize>) -> ToolTable {
    ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_timed(
            spec("srv:skill/index", "唯一一个开局工具"),
            CallTiming::SessionStart,
            counting_ok(counter, MARKER),
        )
}

/// 验收对应「快照 → 恢复 → 子的下一轮请求体过滤行为不变（值从状态来）」。
/// 分两段：①`prefix_allowed_of`/`prefix_chunks` 逐字节复原、`SessionStart`
/// 不偷跑（状态本身没丢/没被重算）；②用复原出的 session 再跑一轮、再 spawn
/// 一个子，wire 上的过滤结果跟恢复之前一致（证明的不只是「值还在」，是
/// 「这套机制读的就是这份复原出来的状态」）。
#[test]
fn prefix_allowed_of_survives_a_snapshot_restore_cycle_and_keeps_filtering_the_wire() {
    let dir = support::temp_dir("inherit-prefix-restore");
    let counter = Arc::new(AtomicUsize::new(0));

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table(Arc::clone(&counter))).expect("唯一的开局工具该成功");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "会话创建这一次该恰好执行一次"
    );

    let wire = spawn_wire();
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call(
            "call_c1",
            &wire,
            &spawn_input(json!({"task": "restore me", "inherit_prefix": []})),
        ),
        support::sse_text("child c1 reported"),
        support::sse_text("root received c1"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, table(Arc::clone(&counter)));
    let status = run_turn(
        &mut session,
        &mut ctx,
        "spawn a child before taking a snapshot",
    )
    .expect("不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let child = AgentId::root().child(1);
    let original_allowed = session.prefix_allowed_of(&child);
    assert_eq!(
        original_allowed,
        Some(Vec::new()),
        "inherit_prefix: [] 该落一个空的允许名单，不是「无限制」（那是 None 的语义）"
    );
    let original_chunks = session.prefix_chunks();

    let snapshot = session.primitives();
    let restored = Session::restore(
        AgentId::root(),
        Some(snapshot),
        Vec::new(),
        0,
        0,
        DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |key| panic!("恢复不该遇到不认识的键：{key:?}"),
    )
    .expect("从刚拿到的快照恢复不该失败");

    assert_eq!(
        restored.prefix_allowed_of(&child),
        original_allowed,
        "PrefixAllowed 是随 spawn 快照落的状态，恢复该逐字节读回，不是重新校验/推导"
    );
    assert_eq!(
        restored.prefix_chunks(),
        original_chunks,
        "session 级前缀块同样该逐字节复原——144/145 两层状态一起复原"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "恢复路径不该偷跑 SessionStart 执行体"
    );

    // 光复原状态值还不够——过滤发生在组请求体的那一刻。用复原出的 session
    // 继续跑一轮：root 再 spawn 一个新子，同样 inherit_prefix: []，断言它的
    // 请求体 system 段依旧不含 MARKER，且执行计数依旧是 1。
    let mut recovered = restored;
    recovered.begin_turn();
    let (port2, bodies2) = support::spawn_recording_server(vec![
        support::sse_tool_call(
            "call_c2",
            &wire,
            &spawn_input(json!({"task": "post restore child", "inherit_prefix": []})),
        ),
        support::sse_text("child c2 reported"),
        support::sse_text("root received c2"),
    ]);
    let (mut ctx2, _events2) = support::build_ctx_with(port2, &dir, table(Arc::clone(&counter)));
    let status = run_turn(
        &mut recovered,
        &mut ctx2,
        "spawn again after restoring the session",
    )
    .expect("恢复之后继续跑不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let bodies2 = bodies2.lock().unwrap();
    let post_restore_child_system = wire_system_text(&bodies2[1]);
    assert!(
        !post_restore_child_system.contains(MARKER),
        "恢复之后再 spawn 的子，inherit_prefix: [] 依旧该过滤掉 init 块：{post_restore_child_system}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "恢复之后继续跑也不该重跑 SessionStart——前缀块从恢复出的状态来，不是重新执行"
    );
}

/// 验收对应「看门狗：spawn 两子跑完一轮，开局工具执行计数 = 1」——独立视角
/// 再钉一遍：子 agent 不重跑 `SessionStart`，即便一轮里连续 spawn 了两个。
#[test]
fn spawning_two_children_in_one_turn_does_not_rerun_session_start() {
    let dir = support::temp_dir("inherit-prefix-watchdog");
    let counter = Arc::new(AtomicUsize::new(0));

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table(Arc::clone(&counter))).expect("唯一的开局工具该成功");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "会话创建这一次该恰好执行一次"
    );

    let wire = spawn_wire();
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call(
            "call_first",
            &wire,
            &spawn_input(json!({"task": "first child task"})),
        ),
        support::sse_text("first child reported"),
        support::sse_tool_call(
            "call_second",
            &wire,
            &spawn_input(json!({"task": "second child task", "inherit_prefix": []})),
        ),
        support::sse_text("second child reported"),
        support::sse_text("root received both children"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, table(Arc::clone(&counter)));

    let status = run_turn(
        &mut session,
        &mut ctx,
        "spawn two children one after another",
    )
    .expect("不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    assert_eq!(
        session.live_agents().len(),
        3,
        "root + 两个子该都真的长出来了，看门狗断言才不是空跑"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "两个子出生并跑完一整轮之后，SessionStart 执行体计数仍该是 1"
    );
}
