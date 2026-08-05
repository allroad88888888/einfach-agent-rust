//! 验收清单第二条：`Timeout` 注入路径。假服务器挂住不回（写完响应头就不再
//! 发任何数据），`RunnerCtx::provider_timeout` 设成毫秒级，到点该走 016 的
//! 重试路（`Notice::Retrying`），重试预算耗尽后落 `Failed(Provider(Retryable))`。

mod support;

use std::time::{Duration, Instant};

use agent_core::{AgentId, ErrorClass, Failure, Notice, Session, TurnStatus};
use agent_runtime::{RunnerEvent, run_turn};

use support::ScriptedResponse;

#[test]
fn provider_call_timeout_retries_then_fails() {
    let dir = support::temp_dir("timeout");
    // `max_retries = 1`：正好两次 `CallProvider`（首次 + 一次重试），服务器
    // 也就只需要挂住两个连接——断言的连接次数跟重试预算一一对应，不留歧义。
    let port = support::spawn_scripted_server(vec![
        ScriptedResponse::HangAfterHeaders,
        ScriptedResponse::HangAfterHeaders,
    ]);
    let (ctx, events) = support::build_ctx(port, &dir);
    let mut ctx = ctx.with_provider_timeout(Duration::from_millis(150));

    let mut session = Session::new(AgentId::root());
    session.set_max_retries(1);

    let start = Instant::now();
    let status = run_turn(&mut session, &mut ctx, "你好");
    let elapsed = start.elapsed();

    assert_eq!(
        status,
        TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable))
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "两次 150ms 超时该在秒级内收尾，实际 {elapsed:?}——像是没真的放弃挂住的连接"
    );

    let events = events.borrow();
    let retrying = events
        .iter()
        .filter(|e| matches!(e, RunnerEvent::Notice(Notice::Retrying { .. })))
        .count();
    assert_eq!(retrying, 1, "预算 1 次重试，该恰好通报一次：{events:#?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunnerEvent::Notice(Notice::Retrying {
                attempt: 1,
                max_retries: 1
            })
        )),
        "{events:#?}"
    );

    // 超时路径不产出 GuardReport——那一轮压根没收到响应，没有 usage 可对账。
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunnerEvent::TurnGuard { .. }))
    );
}
