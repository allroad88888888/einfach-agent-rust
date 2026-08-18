//! 独立测试：只依据 `docs/issues/146-intercept-registry.md`「验收」「注意」两节 +
//! `docs/ROADMAP.md` §一 决策 29 + 公开签名（`rg "pub fn register_session_tool|
//! pub type SessionToolFn"` 探得的那一行）写成，**不看** `crates/agent-runtime/src/`
//! 的任何实现体（`dispatch.rs`/`ctx.rs` 尤其）。实现由另一个 agent 并行写，本文件
//! 与它互不通信；未落地时编译/断言红是预期结果。
//!
//! # 被测契约（公开层，任务描述里给定，不是从实现体摸出来的）
//!
//! ```ignore
//! pub type SessionToolFn =
//!     Box<dyn Fn(&mut Session, &AgentId, &serde_json::Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>;
//! impl RunnerCtx {
//!     pub fn register_session_tool(&mut self, name: Arc<str>, f: SessionToolFn);
//! }
//! ```
//!
//! # 本文件管一件事：注册的闭包真的跑起来之后，三条出路各自对不对
//!
//! 1. 纯读工具：模型脚本化调用 → 下一轮请求体里带上闭包产出的哨兵串（结果真的
//!    进了 prompt 历史，不是拼在当前这一跳里）。
//! 2. 写状态工具：闭包经 `Session::replace_send_plan`（安全动词，不碰
//!    `set_prefix_chunks`）落一条 journaled entry，`history_len` +1，label 可辨认。
//! 3. `Err` 路：`is_error` 回执，轮正常收尾（不卡在 `ToolsPending`）。
//!
//! 注册机制本身的边界（未声明/未注册名字、撞名 panic、不用时字节不变）在姊妹
//! 文件 `intercept_registry_indep_guards.rs`——两份文件按「闭包跑起来之后 / 注册
//! 这件事的边界」两个不重叠的职责拆开，共用的 spec/助手函数住
//! `intercept_registry_indep_support`。
//!
//! 手法：`support::spawn_scripted_server`/`spawn_recording_server` + `sse_tool_call`/
//! `sse_text` 编排两跳对话（照 `inherit_prefix_indep.rs` 的既有用法），工具表用
//! `ToolTable::with_host_tools` 挂一个测试假 spec（`declares()` 为真但跟真实
//! 位置/可逆性判定无关——截获式工具在 dispatch 里被拦截，从不走到执行器）。

use std::sync::{Arc, Mutex};

use agent_core::{AgentId, Reversibility, Session, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_turn, Aftermath, ToolTable};

use crate::intercept_registry_indep_support::{
    install, spec, tool_result, ERR_SENTINEL, ERR_TOOL, TREE_SENTINEL, TREE_TOOL, WRITE_TOOL,
};
use crate::support;

/// 验收 1：注册一个纯读工具，闭包读 `session.agent_tree()` 拼出带哨兵串 + 真实
/// task 文本的结果，模型脚本化调用它，结果作为 tool_result 出现在下一轮请求体里。
#[test]
fn a_registered_pure_read_tool_reaches_the_next_prompt_via_tool_result() {
    let dir = support::temp_dir("intercept-registry-pure-read");
    let wire = wire_name::to_wire(TREE_TOOL);
    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_tool_call("call_tree", &wire, "{}"),
        support::sse_text("done reading the tree"),
    ]);
    let tools = ToolTable::builtin().with_host_tools(vec![(
        spec(TREE_TOOL, "测试假工具：读 agent_tree 拼文本"),
        Reversibility::Pure,
    )]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    install(
        &mut ctx,
        TREE_TOOL,
        Box::new(|session: &mut Session, agent: &AgentId, _input: &serde_json::Value| {
            let tree = session.agent_tree();
            let task = tree
                .nodes
                .iter()
                .find(|n| &n.id == agent)
                .and_then(|n| n.task.clone())
                .unwrap_or_default();
            // 201：纯读，什么都没碰 → `Aftermath::Nothing`。
            Ok((Arc::from(format!("{TREE_SENTINEL} task={task}")), Aftermath::Nothing))
        }),
    );

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "please read the tree TASK-ALPHA-9f2")
        .expect("scripted turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let (result_text, is_error) = tool_result(&session, &AgentId::root(), "call_tree");
    assert!(!is_error, "纯读不该失败：{result_text}");
    assert!(
        result_text.contains(TREE_SENTINEL) && result_text.contains("TASK-ALPHA-9f2"),
        "闭包该读到真实的 session 状态而不是写死的话：{result_text}"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "两跳：工具调用一跳 + 收尾文本一跳");
    assert!(
        bodies[1].contains(TREE_SENTINEL),
        "下一轮请求体该带上 tool_result 里的哨兵串：{}",
        bodies[1]
    );
}

/// 验收 2：闭包经 `Session::replace_send_plan`（per-agent 安全动词，不是
/// `set_prefix_chunks`）写一条状态：`history_len` 恰好 +1，且新 entry 的 label
/// 可辨认（就是这条命令自己的名字），证明写走的是 command 面而不是裸 `store.set`。
#[test]
fn a_registered_tool_writes_through_the_command_surface_and_leaves_one_labeled_entry() {
    let dir = support::temp_dir("intercept-registry-write");
    let wire = wire_name::to_wire(WRITE_TOOL);
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_write", &wire, "{}"),
        support::sse_text("plan advanced"),
    ]);
    let tools = ToolTable::builtin().with_host_tools(vec![(
        spec(WRITE_TOOL, "测试假工具：经 command 面写 send_plan"),
        Reversibility::Reversible,
    )]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    // 探针记的是「第几次调用 / 第几个参数 / 哪个工具名」三元组，只在这个测试里活着。
    type Probe = Arc<Mutex<Option<(usize, usize, &'static str)>>>;
    let probe: Probe = Arc::new(Mutex::new(None));
    let probe_captured = Arc::clone(&probe);
    install(
        &mut ctx,
        WRITE_TOOL,
        Box::new(move |session: &mut Session, agent: &AgentId, _input: &serde_json::Value| {
            let before = session.history_len();
            let mut plan = session.send_plan_of(agent);
            plan.advance_boundary(1, None)
                .expect("pristine plan 从边界 0 前进到 1 不该被拒");
            session.replace_send_plan(agent, plan);
            let after = session.history_len();
            let label = session.last_entry().expect("刚写完该有一条 entry").meta.label;
            *probe_captured.lock().unwrap() = Some((before, after, label));
            // 201：只写了**状态**（走 command 面、进日志），外部世界没碰过——
            // 状态回滚就够了，所以是 `Nothing` 而不是交一个还原函数。
            Ok((Arc::from("plan advanced"), Aftermath::Nothing))
        }),
    );

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "advance the send plan")
        .expect("scripted turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let (result_text, is_error) = tool_result(&session, &AgentId::root(), "call_write");
    assert!(!is_error, "写状态成功不该报 is_error：{result_text}");

    let (before, after, label) = probe.lock().unwrap().expect("闭包该真的跑过一次");
    assert_eq!(
        after,
        before + 1,
        "经 command 面写一次该恰好落一条 journaled entry"
    );
    assert_eq!(
        label, "replace_send_plan",
        "entry 的 label 该可辨认是哪条命令写的，不是一句空话"
    );
}

/// 验收 3：闭包返回 `Err` → 结果落 `is_error`，且这一轮仍然正常收尾（不卡在
/// `ToolsPending`）——003 的老哲学：工具失败不中止 loop。
#[test]
fn a_registered_tool_returning_err_is_reported_and_the_turn_still_ends_cleanly() {
    let dir = support::temp_dir("intercept-registry-err");
    let wire = wire_name::to_wire(ERR_TOOL);
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_err", &wire, "{}"),
        support::sse_text("acknowledged the failure, moving on"),
    ]);
    let tools = ToolTable::builtin()
        .with_host_tools(vec![(spec(ERR_TOOL, "测试假工具：总是失败"), Reversibility::Pure)]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    install(
        &mut ctx,
        ERR_TOOL,
        Box::new(|_session: &mut Session, _agent: &AgentId, _input: &serde_json::Value| {
            Err(Arc::from(ERR_SENTINEL))
        }),
    );

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call the failing tool")
        .expect("scripted turn should not be a source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "Err 路不该让 loop 卡住——该正常收尾"
    );

    let (result_text, is_error) = tool_result(&session, &AgentId::root(), "call_err");
    assert!(is_error, "闭包返回 Err 该落 is_error：{result_text}");
    assert!(
        result_text.contains(ERR_SENTINEL),
        "错误文本该原样透传给模型：{result_text}"
    );
}
