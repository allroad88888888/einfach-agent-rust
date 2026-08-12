//! 独立测试：只依据 `docs/issues/145-spawn-inherit-prefix.md`「验收」「注意」
//! 两节 + `docs/ROADMAP.md` §一 决策 28 + 公开 API
//! `agent_runtime::{ToolTable, CallTiming, run_session_start, run_turn}` 与
//! `agent_core::Session` 的公开面（含 144 刚加的 `prefix_allowed_of`）写成，
//! **不看** `crates/agent-runtime/src/` 与 `crates/agent-core/src/` 下的任何
//! 实现体。实现由另一个 agent 并行写，本文件与它互不通信；未落地时编译/断言
//! 红是预期结果。
//!
//! 本文件单管「合法的 `inherit_prefix` 值怎么过滤」：缺省 / `[]` / 具名列表
//! 三档语义在 wire 上真的表现为子请求体 system 段有/无对应 `init:` 块，具名
//! 列表读回时排序去重。非法名字的从严校验在姊妹文件
//! `inherit_prefix_rejects_indep.rs`；快照恢复与看门狗计数在
//! `inherit_prefix_restore_indep.rs`——三份文件按「合法值过滤 / 非法值拒绝 /
//! 状态稳不稳」三个不重叠的职责拆开。
//!
//! 手法：`run_session_start` 产出前缀块，照 `session_start_indep.rs` /
//! `session_start_prompt_indep.rs` 的 timed 工具构造手法；假 provider 脚本化
//! `srv:agent/spawn` 往返，`support::spawn_recording_server` /
//! `spawn_scripted_server` 录制/驱动原始请求体（照 `session_start_prompt_indep.rs`
//! 看「进 prompt」那一半），system 段抽取照
//! `skill_switch_wire_indep.rs::wire_system_text`；工具名转义用
//! `agent_providers::wire_name::to_wire`，不手拼转义结果（同文件的取舍）。一次
//! blocking spawn 在 wire 上是三条连接：父首跳（吐出 spawn 的 tool_call）、子的
//! 首跳（跟 `happy_two_hop.rs` 的两跳收敛同一机制，只是中间换成另一个 agent 在
//! 打）、父收到 tool_result 后的收尾跳。多次 spawn 分多轮跑（`begin_turn` 隔开，
//! 照 `session_start_prompt_indep.rs` 的多轮手法），不引入并行 needle 路由——
//! 每轮只有一个 spawn，连接顺序天然确定。

use std::sync::Arc;

use agent_core::{AgentId, AgentLimits, Session, ToolSpec, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_session_start, run_turn, CallTiming, TimedRun, ToolTable};
use serde_json::json;

use crate::support;

const MARKER: &str = "INHERIT-PREFIX-INIT-MARKER-91cd";

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

/// `srv:agent/spawn` 的入参，编成 `support::sse_tool_call` 的 `arguments`
/// 参数要的原文：那是 wire 上 `function.arguments`**字符串字段**的内容，
/// 所以 JSON 对象先 `to_string()`，再整体当一个 JSON 字符串转义一次（照
/// `support::sse_tool_call` 自己的文档注释：「里面的引号要按 JSON 字符串再
/// 转义一次」）——不手拼转义结果，避免手误漏转一层引号。
fn spawn_input(value: serde_json::Value) -> String {
    let raw = value.to_string();
    let escaped = serde_json::to_string(&raw).expect("字符串序列化不该失败");
    escaped[1..escaped.len() - 1].to_string()
}

/// 一张带一个 `SessionStart` 索引工具、带 spawn 能力的工具表——三条 wire 级
/// 测试共用的开局装配。
fn table_with_index_tool_and_spawn() -> ToolTable {
    ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_timed(
            spec("srv:skill/index", "会话开局索引，带一段标记文本"),
            CallTiming::SessionStart,
            ok_text(MARKER),
        )
}

/// 验收对应「线级：缺省 → 子请求体 system 含 init 块文本；`[]` → 不含；两个
/// `[]` 姊妹的 system 段逐字节相同」——三档语义分三轮跑（同一个 session 连续
/// 三轮 spawn，`begin_turn` 隔开）：轮一缺省、轮二/三都是 `inherit_prefix: []`。
#[test]
fn wire_level_default_and_empty_inherit_prefix_control_the_init_block() {
    let dir = support::temp_dir("inherit-prefix-wire");
    let tools = table_with_index_tool_and_spawn();

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("唯一的开局工具该成功");

    let wire = spawn_wire();
    let (port, bodies) = support::spawn_recording_server(vec![
        // 轮 1：缺省 inherit_prefix（不填字段）。
        support::sse_tool_call(
            "call_default",
            &wire,
            &spawn_input(json!({"task": "default child task"})),
        ),
        support::sse_text("default child reported"),
        support::sse_text("root received the default child"),
        // 轮 2：inherit_prefix: []（姊妹 A）。
        support::sse_tool_call(
            "call_empty_a",
            &wire,
            &spawn_input(json!({"task": "empty sibling A task", "inherit_prefix": []})),
        ),
        support::sse_text("empty sibling A reported"),
        support::sse_text("root received sibling A"),
        // 轮 3：inherit_prefix: []（姊妹 B）——跟轮 2 除 task 文本外该同形。
        support::sse_tool_call(
            "call_empty_b",
            &wire,
            &spawn_input(json!({"task": "empty sibling B task", "inherit_prefix": []})),
        ),
        support::sse_text("empty sibling B reported"),
        support::sse_text("root received sibling B"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    let status =
        run_turn(&mut session, &mut ctx, "spawn a default child").expect("第一轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    session.begin_turn();
    let status = run_turn(&mut session, &mut ctx, "spawn an empty-prefix sibling A")
        .expect("第二轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    session.begin_turn();
    let status = run_turn(&mut session, &mut ctx, "spawn an empty-prefix sibling B")
        .expect("第三轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        9,
        "三轮各三条连接（父首跳/子/父收尾跳）：{}",
        bodies.len()
    );

    // 每轮里「子」自己的那条连接排第二（下标 1、4、7）。
    let default_child_system = wire_system_text(&bodies[1]);
    let empty_a_system = wire_system_text(&bodies[4]);
    let empty_b_system = wire_system_text(&bodies[7]);

    assert_eq!(
        default_child_system.matches(MARKER).count(),
        1,
        "缺省 inherit_prefix：子该带 init 块，标记该恰出现一次：{default_child_system}"
    );
    assert!(
        !empty_a_system.contains(MARKER),
        "inherit_prefix: []：子不该带 init 块：{empty_a_system}"
    );
    assert!(
        !empty_b_system.contains(MARKER),
        "inherit_prefix: []：子不该带 init 块：{empty_b_system}"
    );
    assert_eq!(
        empty_a_system, empty_b_system,
        "两个 inherit_prefix: [] 的姊妹，system 段该逐字节相同——缓存共享性质保住"
    );
}

/// 验收对应「合法列名（有 `SessionStart` 工具的表）→
/// `prefix_allowed_of(child)` 读回排序去重名单」，顺带验证名单真的驱动了
/// wire 上的过滤——不是校验通过了但没接到 `system_for`。
#[test]
fn a_legal_named_list_is_read_back_sorted_and_deduped_and_filters_the_wire_system_segment() {
    let dir = support::temp_dir("inherit-prefix-partial");
    const INDEX_MARKER: &str = "PARTIAL-INDEX-MARKER-a1";
    const ROSTER_MARKER: &str = "PARTIAL-ROSTER-MARKER-b2";
    const CHANGELOG_MARKER: &str = "PARTIAL-CHANGELOG-MARKER-c3";

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_timed(
            spec("srv:skill/index", "索引"),
            CallTiming::SessionStart,
            ok_text(INDEX_MARKER),
        )
        .with_timed(
            spec("srv:skill/roster", "名册"),
            CallTiming::SessionStart,
            ok_text(ROSTER_MARKER),
        )
        .with_timed(
            spec("srv:skill/changelog", "变更日志"),
            CallTiming::SessionStart,
            ok_text(CHANGELOG_MARKER),
        );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("三个开局工具都该成功");

    let wire = spawn_wire();
    // 顺序打乱 + 重复：验收要求「排序去重名单」，光给一份规整输入测不出这条。
    let input = spawn_input(json!({
        "task": "pick roster and index only",
        "inherit_prefix": ["srv:skill/roster", "srv:skill/index", "srv:skill/roster"],
    }));
    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_tool_call("call_partial", &wire, &input),
        support::sse_text("partial child reported"),
        support::sse_text("root received the partial child"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    let status = run_turn(&mut session, &mut ctx, "spawn with a partial named inherit_prefix")
        .expect("合法名单不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let child = AgentId::root().child(1);
    let allowed = session
        .prefix_allowed_of(&child)
        .expect("挑着带的子该有 PrefixAllowed");
    let allowed_names: Vec<&str> = allowed.iter().map(|s| &**s).collect();
    assert_eq!(
        allowed_names,
        vec!["srv:skill/index", "srv:skill/roster"],
        "排序去重：重复的 roster 只留一份，且按名字排序"
    );

    let bodies = bodies.lock().unwrap();
    let child_system = wire_system_text(&bodies[1]);
    assert!(
        child_system.contains(INDEX_MARKER),
        "index 在名单里，子该带：{child_system}"
    );
    assert!(
        child_system.contains(ROSTER_MARKER),
        "roster 在名单里，子该带：{child_system}"
    );
    assert!(
        !child_system.contains(CHANGELOG_MARKER),
        "changelog 不在名单里，子不该带：{child_system}"
    );
}
