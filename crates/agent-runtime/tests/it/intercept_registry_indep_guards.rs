//! 独立测试：`intercept_registry_indep.rs` 的姊妹文件，同一份契约（见那份文件的
//! 头注释），同样**不看**实现体。本文件管另一件事：注册机制本身的边界，跟「闭包
//! 跑起来之后对不对」（那份文件的职责）不重叠。
//!
//! # 本文件管的三条验收
//!
//! 4. 未声明/未注册的名字：`is_error` + loop 不挂（文案宽松，不比对具体措辞）
//!    ——跟这条 issue 落地之前一模一样，走既有的 unknown_tool 路。
//! 5. 注册一个表里没有 spec 的名字：debug 构建 panic（撞名判据的镜像：一名一路，
//!    缺一不可）。
//! 6. 不碰这个新 API 的会话：第一轮请求体字节不受"某处注册了一个从未被调用的
//!    截获式工具"影响（红线 11：机制未使用时一字节都不该变）。

use std::sync::Arc;

use agent_core::{AgentId, Session, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_turn, Aftermath, ToolTable};

use crate::intercept_registry_indep_support::{install, tool_result, GHOST_TOOL, UNKNOWN_TOOL};
use crate::support;

/// 验收 4：既没声明也没注册的名字——跟这条 issue 之前一模一样，走 unknown_tool
/// 路：`is_error` + loop 不挂。文案宽松（不比对具体措辞），只断这两条硬事实。
#[test]
fn a_name_that_is_neither_declared_nor_registered_still_gets_unknown_tool_treatment() {
    let dir = support::temp_dir("intercept-registry-unknown");
    let ghost_wire = wire_name::to_wire(UNKNOWN_TOOL);
    let tools = ToolTable::builtin();
    assert!(
        !tools.declares(UNKNOWN_TOOL),
        "这条测试的前提就是表里没有这个名字"
    );
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_ghost", &ghost_wire, "{}"),
        support::sse_text("that tool does not exist, trying something else"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call a made-up tool")
        .expect("scripted turn should not be a source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "未注册的名字不该把 loop 挂住"
    );

    let (_result_text, is_error) = tool_result(&session, &AgentId::root(), "call_ghost");
    assert!(is_error, "未声明/未注册的名字该落 is_error（unknown_tool 语义）");
}

/// 验收 5：注册一个工具表里根本没有 spec 的名字——debug 构建下必须 panic。
/// `#[cfg(debug_assertions)]` 门住整条测试，跟 `call_timing_indep.rs` 的同款
/// 判据（`with_timed` 撞名 panic）用同一个理由：release 构建里 `debug_assert!`
/// 是空操作，测这条会误红。
#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn registering_a_name_the_table_never_declared_panics_in_debug() {
    let dir = support::temp_dir("intercept-registry-panic");
    let port = support::spawn_scripted_server(vec![]);
    assert!(
        !ToolTable::builtin().declares(GHOST_TOOL),
        "前提：这个名字确实不在 builtin() 表里"
    );
    let (mut ctx, _events) = support::build_ctx(port, &dir);
    install(
        &mut ctx,
        GHOST_TOOL,
        Box::new(|_session: &mut Session, _agent: &AgentId, _input: &serde_json::Value| {
            Ok((Arc::from("unreachable"), Aftermath::Nothing))
        }),
    );
}

/// 验收 6（红线 11 看门狗）：一个完全不碰 `register_session_tool` 的会话，跟一个
/// "在别处注册了一个从未被模型调用的截获式工具"的会话，第一轮请求体逐字节相同
/// ——注册这件事本身住在 `RunnerCtx`，不进 `ToolTable::specs()`，机制存在但没被
/// 调用时，prompt 一个字节都不该变。
#[test]
fn a_session_that_never_touches_the_registry_gets_byte_identical_first_round_bytes() {
    let dir_a = support::temp_dir("intercept-registry-baseline-a");
    let dir_b = support::temp_dir("intercept-registry-baseline-b");

    let (port_a, bodies_a) =
        support::spawn_recording_server(vec![support::sse_text("plain reply, no tools")]);
    let (mut ctx_a, _events_a) = support::build_ctx(port_a, &dir_a);
    let mut session_a = Session::new(AgentId::root());
    let status_a = run_turn(&mut session_a, &mut ctx_a, "hello baseline")
        .expect("scripted turn should not be a source failure");
    assert_eq!(status_a, TurnStatus::Done { truncated: false });

    let (port_b, bodies_b) =
        support::spawn_recording_server(vec![support::sse_text("plain reply, no tools")]);
    let (mut ctx_b, _events_b) = support::build_ctx(port_b, &dir_b);
    // 注册在一个 builtin() 已声明的名字上（"srv:fs/read"），但这一轮模型根本不调
    // 它——只用来证明"注册过、但没被调用"不改变第一轮请求体。
    install(
        &mut ctx_b,
        "srv:fs/read",
        Box::new(|_session: &mut Session, _agent: &AgentId, _input: &serde_json::Value| {
            Ok((Arc::from("stub-unused"), Aftermath::Nothing))
        }),
    );
    let mut session_b = Session::new(AgentId::root());
    let status_b = run_turn(&mut session_b, &mut ctx_b, "hello baseline")
        .expect("scripted turn should not be a source failure");
    assert_eq!(status_b, TurnStatus::Done { truncated: false });

    let bodies_a = bodies_a.lock().unwrap();
    let bodies_b = bodies_b.lock().unwrap();
    assert_eq!(bodies_a.len(), 1);
    assert_eq!(bodies_b.len(), 1);
    assert_eq!(
        bodies_a[0], bodies_b[0],
        "红线 11：注册一个未被调用的截获式工具，不该改动第一轮请求体的任何字节"
    );
}
