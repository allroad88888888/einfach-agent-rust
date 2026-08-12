//! 独立测试：只依据 `docs/issues/145-spawn-inherit-prefix.md`「验收」「注意」
//! 两节 + `docs/ROADMAP.md` §一 决策 28 + 公开 API
//! `agent_runtime::{ToolTable, CallTiming, run_session_start, run_turn}` 与
//! `agent_core::Session` 的公开面写成，**不看** `crates/agent-runtime/src/` 与
//! `crates/agent-core/src/` 下的任何实现体。实现由另一个 agent 并行写，本文件
//! 与它互不通信；未落地时编译/断言红是预期结果。
//!
//! 本文件单管一件事：`inherit_prefix` 点名一个不在 `timed(SessionStart)` 里
//! 的名字时的从严校验——`is_error`、不长树、父轮正常收尾。合法值（缺省/`[]`/
//! 合法具名列表）怎么过滤在姊妹文件 `inherit_prefix_indep.rs`；快照恢复与看
//! 门狗计数在 `inherit_prefix_restore_indep.rs`。三份文件按「合法值过滤 / 非法
//! 值拒绝 / 状态稳不稳」三个不重叠的职责拆开。
//!
//! 手法照 `inherit_prefix_indep.rs`：`run_session_start` 产出前缀块（照
//! `session_start_indep.rs` 的 timed 工具构造手法），假 provider 脚本化
//! `srv:agent/spawn` 往返（`support::spawn_scripted_server`），tool_result 提取
//! 手法照 `skill_switch_wire_indep.rs::tool_results`，工具名转义用
//! `agent_providers::wire_name::to_wire`。

use std::sync::Arc;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, ToolSpec, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_session_start, run_turn, CallTiming, TimedRun, ToolTable};
use serde_json::json;

use crate::support;

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 总是成功、回一段固定文本的执行体——照 `session_start_indep.rs::ok_text`。
fn ok_text(text: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable,
              _session: &Session,
              _input: &serde_json::Value|
              -> Result<Arc<str>, Arc<str>> { Ok(Arc::from(text)) },
    )
}

/// 会话历史里全部 `ToolResult` 的 (正文, is_error)。手法照抄
/// `skill_switch_wire_indep.rs::tool_results`。
fn tool_results(session: &Session) -> Vec<(String, bool)> {
    session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .collect()
}

/// `srv:agent/spawn` 的入参，编成 `support::sse_tool_call` 的 `arguments` 参数
/// 要的原文——照 `inherit_prefix_indep.rs::spawn_input` 同一个手法：JSON 对象
/// `to_string()` 之后再当一个 JSON 字符串整体转义一次。
fn spawn_input(value: serde_json::Value) -> String {
    let raw = value.to_string();
    let escaped = serde_json::to_string(&raw).expect("字符串序列化不该失败");
    escaped[1..escaped.len() - 1].to_string()
}

/// 验收对应「`inherit_prefix: ["不存在的名字"]` → 父收到 `is_error`（文案可
/// 辨认非法项）、树上没有长出子、父轮正常收尾」。
#[test]
fn an_unknown_name_in_inherit_prefix_is_rejected_before_any_child_is_created() {
    let dir = support::temp_dir("inherit-prefix-illegal");
    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_timed(
            spec("srv:skill/index", "唯一一个合法的开局工具"),
            CallTiming::SessionStart,
            ok_text("index body"),
        );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("唯一的开局工具该成功");

    let wire = wire_name::to_wire("srv:agent/spawn");
    const ILLEGAL: &str = "srv:skill/does-not-exist";
    let input = spawn_input(json!({
        "task": "try an illegal inherit_prefix name",
        "inherit_prefix": [ILLEGAL],
    }));
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_illegal", &wire, &input),
        support::sse_text("root saw the refusal and moved on"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    let status = run_turn(
        &mut session,
        &mut ctx,
        "spawn with an illegal inherit_prefix name",
    )
    .expect("非法项该走 is_error 的 tool_result，不该是 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "003 哲学：工具失败不中止 loop，父轮该正常收尾"
    );

    assert_eq!(
        session.live_agents(),
        vec![AgentId::root()],
        "名字不在 timed 区 → 整次 spawn 不发生，树上不该长出任何子"
    );

    let results = tool_results(&session);
    assert_eq!(results.len(), 1, "该只有一条 tool_result：{results:#?}");
    assert!(results[0].1, "非法名字该落 is_error：{results:#?}");
    assert!(
        results[0].0.contains(ILLEGAL),
        "错误文案该点名非法项，才谈得上「可辨认」：{}",
        results[0].0
    );
}
