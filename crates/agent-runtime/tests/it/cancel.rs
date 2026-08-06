//! 验收清单第三条：取消路径。假服务器挂住不回，一个后台线程模拟 Ctrl-C——
//! 200ms 后把 `RunnerCtx::cancel_flag()` 置位，`run_turn` 该落
//! `Failed(Cancelled)` 终态，且不是靠我们自己的超时机制凑巧撞上（超时预算
//! 特意设得远大于这条测试的时间尺度）。

use crate::support;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use agent_core::{AgentId, Failure, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::support::ScriptedResponse;

#[test]
fn ctrl_c_during_call_provider_cancels_the_turn() {
    let dir = support::temp_dir("cancel");
    let port = support::spawn_scripted_server(vec![ScriptedResponse::HangAfterHeaders]);
    let (ctx, _events) = support::build_ctx(port, &dir);
    // 超时预算拉得远大于这条测试的时间尺度：观察到的终态必须是取消标志起的
    // 作用，不是我们自己的超时机制抢跑撞上同一个终态巧合看起来一样。
    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(5));

    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    let mut session = Session::new(AgentId::root());
    let start = Instant::now();
    let status = run_turn(&mut session, &mut ctx, "你好").expect("cancellation is not a source failure");
    let elapsed = start.elapsed();

    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert!(session.tool_slots().is_empty(), "016：取消要把槽位全弃");
    assert!(
        elapsed < Duration::from_secs(2) && elapsed >= Duration::from_millis(200),
        "该在置位之后的几个 poll 间隔内收尾，不该等到 5s 的超时预算，实际 {elapsed:?}"
    );
}
