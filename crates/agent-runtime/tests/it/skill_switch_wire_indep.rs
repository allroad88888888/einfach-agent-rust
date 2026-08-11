//! 139 独立测试（线级半）：只依据 `docs/issues/139-skill-assembly-switch.md`
//! 「验收」「注意」两节 + `docs/INVARIANTS.md` 红线 11 + 公开 API
//! `agent_runtime::{ToolTable, SkillRegistry, run_session_start, run_turn}` 写成，
//! **不看** `crates/agent-runtime/src/tool_table_skill.rs` / `skill/` 目录里的
//! 实现体。实现由另一个 agent 并行写，本文件与它互不通信；切换尚未落地时编译/
//! 断言红是预期结果。
//!
//! 本文件管**字节怎么上线**——假 provider 脚本化一次「模型调 `srv:skill/read`」
//! 的往返，断言正文只经 tool_result 追加到消息尾、system 段全程只有索引；以及
//! 模型猜 `srv:skill/activate` 时走 unknown_tool 的 is_error 回执、loop 不挂。
//! 静态装配形状（`declares`/`specs`/`timed`/`run_session_start` 产出的前缀块内容）
//! 在 `skill_switch_indep.rs`。
//!
//! wire 名字转义走 `agent_providers::wire_name::to_wire`——不在本文件里手拼
//! `srv_3Askill_2Fread` 这类转义结果，理由跟
//! `remote_undeclared_tool_is_not_a_hang.rs` 一致：转义规则属于 provider 层，
//! 独立测试不该复刻第二遍。

use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, HostSkill, Session, SkillId, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_session_start, run_turn, SkillRegistry, ToolTable};

use crate::support;

fn skill(id: &str, description: &str, body: &str) -> HostSkill {
    HostSkill {
        id: SkillId::new(id),
        description: Arc::from(description),
        body: Arc::from(body),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }
}

/// 会话历史里全部 `ToolResult` 的 (正文, is_error)。
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

/// 请求体里那条 `role: "system"` 消息的正文——模型真正看到的那串字符。
/// 手法照抄 `session_start_prompt_indep.rs::wire_system_text`。
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

/// 验收「首轮 encode body：system 含索引块、不含任何 skill 正文字节；正文只出现
/// 在 read 的 tool_result 里」：假 provider 第一跳脚本化返回一个
/// `srv:skill/read` 的 tool_call（入参是 fixture 里的 skill id）；两跳后收敛
/// （read 是本地同步 `Pure` 工具，不占等待槽，跟 `happy_two_hop.rs` 的
/// `srv:fs/read` 同一种收敛形状）。断言：第一轮请求体的 system 段含索引、不含
/// 正文哨兵串；第二轮**整条**请求体含正文哨兵串（经 tool_result 带上）；第二轮
/// system 段仍然不含正文哨兵串、且与第一轮逐字节相同（read 不改前缀）。
#[test]
fn a_scripted_read_tool_call_lands_its_body_only_in_the_tool_result_never_in_system() {
    const SKILL_ID: &str = "wire-read-flow";
    const SENTINEL: &str = "SKILL_BODY_SENTINEL_QX52";

    let dir = support::temp_dir("skill-switch-wire-read-fs");
    let registry = SkillRegistry::from_host_skills(vec![skill(
        SKILL_ID,
        "线级读正文验收用的流程",
        &format!("这是 {SKILL_ID} 的正文。{SENTINEL}"),
    )]);
    let table = ToolTable::builtin().with_skills(registry);

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("非空 registry 的索引工具不该失败");

    let arguments = format!(r#"{{\"skill\": \"{SKILL_ID}\"}}"#);
    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_tool_call(
            "call_read",
            &wire_name::to_wire("srv:skill/read"),
            &arguments,
        ),
        support::sse_text("已经读到内容了"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, table);

    let status = run_turn(&mut session, &mut ctx, "读一下 wire-read-flow 的正文")
        .expect("read 是本地同步工具，不该是 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "一次工具调用 + 一次收敛，该正好两跳跑完"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "该正好录到两条请求体，实际: {}", bodies.len());

    let first_system = wire_system_text(&bodies[0]);
    assert!(
        first_system.contains(SKILL_ID),
        "首轮 system 段该含索引里的 skill id: {first_system}"
    );
    assert!(
        !first_system.contains(SENTINEL),
        "首轮 system 段不该含任何正文字节: {first_system}"
    );

    assert!(
        bodies[1].contains(SENTINEL),
        "第二轮请求体该以 tool_result 形式带上正文哨兵串: {}",
        bodies[1]
    );
    let second_system = wire_system_text(&bodies[1]);
    assert!(
        !second_system.contains(SENTINEL),
        "第二轮 system 段仍不该含正文字节——正文只出现在 tool_result 里: {second_system}"
    );
    assert_eq!(
        second_system, first_system,
        "read 不该改动 system 段的字节——前缀不能因为一次 read 就漂了（红线 11）"
    );
}

/// 验收「模型猜 activate：脚本化返回一个 srv:skill/activate 的 tool_call → 走
/// unknown_tool 路（is_error 回执），loop 不挂」：切换之后 `srv:skill/activate`
/// 不再被 `declares()`，模型编出来的调用跟任何未声明的工具走同一条路
/// （`remote_undeclared_tool_is_not_a_hang.rs` 已经钉住的那条：`ctx.fs.execute`
/// 的 `unknown_tool` → `is_error` 的 tool_result，两跳后正常收敛）。
#[test]
fn a_scripted_activate_tool_call_is_treated_as_unknown_and_does_not_hang_the_loop() {
    const SKILL_ID: &str = "wire-activate-flow";

    let dir = support::temp_dir("skill-switch-wire-activate-fs");
    let registry = SkillRegistry::from_host_skills(vec![skill(
        SKILL_ID,
        "线级猜测 activate 验收用的流程",
        "这段正文不该被这条测试读到。",
    )]);
    let table = ToolTable::builtin().with_skills(registry);
    assert!(
        !table.declares("srv:skill/activate"),
        "这条测试的前提：切换之后表里不该再有 activate"
    );

    let arguments = format!(r#"{{\"skill\": \"{SKILL_ID}\"}}"#);
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call(
            "call_guess_activate",
            &wire_name::to_wire("srv:skill/activate"),
            &arguments,
        ),
        support::sse_text("这个工具已经不在了，我换个办法。"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, table);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "帮我激活 wire-activate-flow")
        .expect("unknown tool 不该是 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "该跑完第二跳收敛，而不是停在 ToolsPending——loop 不挂"
    );

    let results = tool_results(&session);
    assert_eq!(results.len(), 1, "该正好有一条 tool_result: {results:#?}");
    assert!(
        results[0].1,
        "模型猜的 activate 该落 is_error（unknown_tool 语义）: {results:#?}"
    );
    assert!(
        results[0].0.contains("unknown_tool"),
        "错误里该说清是「不认识这个工具」: {results:#?}"
    );
}
