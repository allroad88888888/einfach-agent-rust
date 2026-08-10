//! 103「定死的接口」新增的 `Session::prev_send_plan_of` 的命令层契约：
//! 对一个从没发过请求的 agent 返回 pristine，不 panic。跟
//! `send_plan_of_session_default.rs` 里 `send_plan_of` 的同名两条测试是同一套
//! 约定（`*_of` 口子：键不在 family 里就落默认值，不 panic）——`PrevSendPlan`
//! 没有理由更特殊。
//!
//! 只看公开签名（103「定死的接口」那节给的方法），不看它的实现体。

use agent_core::{AgentId, SendPlan};

use crate::support::session::new_session;

/// 从没调用过任何压缩 command 的 root：`prev_send_plan_of` 返回 pristine
/// （`SendPlan::new()`），不是 panic、不是某个未初始化的占位值。
#[test]
fn prev_send_plan_of_a_never_touched_root_is_pristine() {
    let session = new_session();
    let plan = session.prev_send_plan_of(&AgentId::root());
    assert!(plan.is_pristine());
    assert_eq!(plan, SendPlan::new());
}

/// 一个语法上合法、但从没被 `spawn_child` 建过图的 `AgentId`：
/// `prev_send_plan_of` 照样返回 pristine，不 panic。
#[test]
fn prev_send_plan_of_an_agent_that_was_never_spawned_does_not_panic() {
    let session = new_session();
    let ghost = AgentId::root().child(99);
    assert!(!session.is_live(&ghost), "这个 id 压根没被 spawn 过");

    let plan = session.prev_send_plan_of(&ghost);
    assert!(plan.is_pristine(), "没建过图的 agent 也该拿到 pristine，而不是 panic");
}
