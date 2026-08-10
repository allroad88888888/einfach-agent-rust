//! 014 缺口 2 的后继（027 换接）：Ctrl-C 全链路验证的 agent-cli 侧集成测试。
//!
//! `agent-runtime/tests/cancel.rs` 已经证过「取消标志置位 → `run_turn` 落
//! `Failed(Cancelled)`」这一段——那是 runner 自己的职责，这条测试不重复。
//! 这里补的是只属于 `agent-cli` 的那一段胶水：`undo::after_cancelled_turn`
//! 怎么处理 `Failed(Cancelled)` 留下的半轮痕迹（027 的正牌答案，取代 022 时代
//! 「截断消息列表」那招——`Session::undo_turn` 干净擦除，不留幽灵消息），
//! 以及「进程逻辑继续」到底是不是真的——下一轮输入还能不能正常处理，而且不会
//! 被上一轮的孤儿消息污染。
//!
//! 手法跟 `agent-runtime/tests/cancel.rs` 一致：假服务器挂住不回，独立线程
//! 200ms 后置位 `RunnerCtx::cancel_flag()`（模拟 Ctrl-C——`main.rs` 里
//! `ctrlc::set_handler` 翻的就是这同一个标志，见该文件顶部注释），不是真的
//! 发一个 SIGINT。

use crate::support;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use agent_cli::undo;
use agent_core::{AgentId, Failure, Role, Session, TurnStatus, UndoReport};
use agent_runtime::run_turn;

use crate::support::ScriptedResponse;

/// 第二跳：普通的一句话回答，跟 `agent-runtime/tests/happy_two_hop.rs` 的
/// `hop2_end_turn` 同一种形状（无工具调用的纯文本收尾），不是这条测试自己
/// 现造的假设。
fn plain_end_turn() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"你好，我在"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
    ])
}

/// repl.rs 每读一行都做的同一件事（终态之后 `begin_turn`，取消就
/// `undo::after_cancelled_turn`）——这里原样重演一遍，不经 stdin，方便测试
/// 直接摆弄状态和断言。
fn one_turn(session: &mut Session, ctx: &mut agent_runtime::RunnerCtx, input: &str) -> TurnStatus {
    if session.status().is_terminal() {
        session.begin_turn();
        agent_runtime::persist::sync(ctx, session);
    }
    let status = run_turn(session, ctx, input)
        .expect("脚本化假服务器不该触发瞬态源失败");
    if matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
        undo::after_cancelled_turn(session, ctx);
    }
    status
}

#[test]
fn cancelled_turn_is_erased_and_the_next_turn_still_works() {
    let dir = support::temp_dir("cancel-flow");
    let port =
        support::spawn_scripted_server(vec![ScriptedResponse::HangAfterHeaders, plain_end_turn()]);
    let mut ctx = support::build_ctx(port, &dir).with_provider_timeout(Duration::from_secs(5));

    // 模拟 Ctrl-C：跟 `main.rs` 里 `ctrlc::set_handler` 翻的是同一份标志
    // （`RunnerCtx::cancel_flag()`），200ms 后置位。超时预算特意拉到 5s，
    // 远大于这条测试的时间尺度——观察到的终态必须是取消标志起的作用。
    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    let mut session = Session::new(AgentId::root());
    let start = Instant::now();
    let status = one_turn(&mut session, &mut ctx, "第一句话，永远等不到回复");
    let elapsed = start.elapsed();

    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert!(
        elapsed < Duration::from_secs(2),
        "该在取消标志置位之后的几个 poll 间隔内收尾，不该等到 5s 超时预算，实际 {elapsed:?}"
    );
    assert!(
        session.messages().is_empty(),
        "自动 undo_turn 该把这一轮的痕迹擦干净：{:#?}",
        session.messages()
    );
    assert_eq!(
        session.status(),
        TurnStatus::Idle,
        "撤到会话开局，本来就是 Idle——不需要额外的 begin_turn"
    );

    // 进程逻辑继续：下一轮输入正常处理，而且历史里只有这一轮的问答——
    // 上一轮被取消的那句话没有借尸还魂。
    let status = one_turn(&mut session, &mut ctx, "第二句话，这次有人接");

    assert_eq!(status, TurnStatus::Done { truncated: false });
    let messages = session.messages();
    assert_eq!(
        messages.len(),
        2,
        "历史该只有这一轮的一问一答：{messages:#?}"
    );
    assert_eq!(messages[0].role, Role::User);
    match &messages[0].blocks[0] {
        agent_core::ContentBlock::Text(t) => {
            assert_eq!(&**t, "第二句话，这次有人接", "不该是被取消那轮的孤儿文本")
        }
        other => panic!("期望 Text，拿到 {other:?}"),
    }
}

/// `/undo` 手动命令走的是同一条 `Session::undo_turn`——这条测试直接验证
/// `undo::undo` 在一次正常收尾的轮次上能把它整个撤掉，`UndoReport` 报的
/// entries/turn_id 跟实际一致。
#[test]
fn manual_undo_after_a_normal_turn_reports_applied_and_erases_it() {
    let dir = support::temp_dir("cancel-flow-manual-undo");
    let port = support::spawn_scripted_server(vec![plain_end_turn()]);
    let mut ctx = support::build_ctx(port, &dir);

    let mut session = Session::new(AgentId::root());
    let status = one_turn(&mut session, &mut ctx, "你好");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert_eq!(session.messages().len(), 2);

    let report = session.undo_turn();
    agent_runtime::persist::sync(&mut ctx, &mut session);
    assert!(
        matches!(report, UndoReport::Applied { turn_id: 1, .. }),
        "{report:?}"
    );
    assert!(session.messages().is_empty());
    assert_eq!(session.status(), TurnStatus::Idle);
}
