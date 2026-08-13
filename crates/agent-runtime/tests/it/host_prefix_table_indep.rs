//! 独立测试：只依据 `docs/issues/155-with-host-prefix.md`「验收」「注意」两节、
//! `docs/INVARIANTS.md` 红线 11、以及公开签名
//! `agent_runtime::ToolTable::with_host_prefix(pairs: &[(Arc<str>, Arc<str>)]) -> ToolTable`
//! 写成，**不看** `crates/agent-runtime/src/tool_table_host_prefix.rs` /
//! `tool_table_host_prefix_tests.rs` 里的实现体。实现由另一个 agent 并行写，
//! 本文件与它互不通信；未落地时编译/断言红是预期结果。
//!
//! 手法照抄既有姊妹文件：`call_timing_indep.rs`（specs/declares/timed 三面的
//! 检查手法）、`session_start_indep.rs`/`skill_switch_indep.rs`
//! （`run_session_start` 产出前缀块的检查手法）、`inherit_prefix_indep.rs`/
//! `inherit_prefix_rejects_indep.rs`（spawn `inherit_prefix` 点名合法/非法名字
//! 的 wire 级验证手法，含 `spawn_input`/`wire_system_text`/`tool_results` 三个
//! 帮助函数的原样复制）。
//!
//! 五条测试对应验收的五个角度（逐条见各测试函数文档注释）：
//! 1. 两对声明 → `run_session_start` 后 `prefix_chunks()` 恰两块，序按 name；
//!    入参数组反序结果不变。
//! 2. 空切片：与不调用逐字节等价（specs/declares/timed 行为三面）。
//! 3. 与内置 timed（`with_skills` 的索引）共存：内置块在前、声明块在后——即使
//!    声明的 name 字母序排在内置块前面，也不该把两批块混在一起重排。
//! 4. `inherit_prefix`（integration 层面）：spawn 点名声明块的名字该成功、点
//!    不存在的名字该拒——照 `inherit_prefix_indep.rs` 的既有惯用法走 pub 面。
//! 5. 合成条目不进模型面：`specs()`/`declares()` 里没有声明的名字。

use std::sync::Arc;

use agent_core::{AgentId, AgentLimits, ContentBlock, HostSkill, Session, SkillId, SystemChunk, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_session_start, run_turn, CallTiming, SkillRegistry, ToolTable};
use serde_json::json;

use crate::support;

fn chunk(label: &str, text: &str) -> SystemChunk {
    SystemChunk {
        label: Arc::from(label),
        text: Arc::from(text),
    }
}

fn pairs(items: &[(&str, &str)]) -> Vec<(Arc<str>, Arc<str>)> {
    items
        .iter()
        .map(|(name, text)| (Arc::from(*name), Arc::from(*text)))
        .collect()
}

/// 请求体里 `role: "system"` 那条消息的正文。手法照抄
/// `inherit_prefix_indep.rs::wire_system_text`。
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

/// `srv:agent/spawn` 的入参，编成 `support::sse_tool_call` 的 `arguments` 参数
/// 要的原文——照 `inherit_prefix_indep.rs::spawn_input` 同一个手法。
fn spawn_input(value: serde_json::Value) -> String {
    let raw = value.to_string();
    let escaped = serde_json::to_string(&raw).expect("字符串序列化不该失败");
    escaped[1..escaped.len() - 1].to_string()
}

/// 会话历史里全部 `ToolResult` 的 (正文, is_error)。手法照抄
/// `inherit_prefix_rejects_indep.rs::tool_results`。
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

/// 验收 1：两对声明（故意乱序喂入：zulu 先、alpha 后）→ `run_session_start`
/// 后 `prefix_chunks()` 恰两块，label `init:<name>`、text 原样、序按 name
/// 排序（不是入参序）；把入参数组反序再来一遍（alpha 先、zulu 后），结果
/// 逐字节不变。
#[test]
fn two_pairs_produce_two_chunks_ordered_by_name_regardless_of_input_order() {
    let forward = pairs(&[
        ("zulu-policy", "Zulu 的文本"),
        ("alpha-policy", "Alpha 的文本"),
    ]);
    let table_forward = ToolTable::empty().with_host_prefix(&forward);
    let mut session_forward = Session::new(AgentId::root());
    run_session_start(&mut session_forward, &table_forward).expect("两条声明都该成功");

    let expected = vec![
        chunk("init:alpha-policy", "Alpha 的文本"),
        chunk("init:zulu-policy", "Zulu 的文本"),
    ];
    assert_eq!(
        session_forward.prefix_chunks(),
        expected,
        "两块，label init:<name>，按 name 排序——不是入参序（zulu 先喂入）"
    );

    let reversed = pairs(&[
        ("alpha-policy", "Alpha 的文本"),
        ("zulu-policy", "Zulu 的文本"),
    ]);
    let table_reversed = ToolTable::empty().with_host_prefix(&reversed);
    let mut session_reversed = Session::new(AgentId::root());
    run_session_start(&mut session_reversed, &table_reversed).expect("两条声明都该成功");

    assert_eq!(
        session_reversed.prefix_chunks(),
        expected,
        "入参数组反序之后，结果必须与正序逐字节相同——排序钉在 name 上，不是入参位置"
    );
}

/// 验收 2：空切片 → 表与不调用 `with_host_prefix` 时逐字节等价——specs()/
/// declares() 可比的两面直接比较，timed 区经行为比（`run_session_start` 后
/// `prefix_chunks()` 为空）。
#[test]
fn an_empty_slice_leaves_the_table_byte_identical_to_not_calling_it() {
    let base = ToolTable::builtin();
    let with_empty = ToolTable::builtin().with_host_prefix(&[]);

    let base_specs_bytes = serde_json::to_vec(base.specs()).expect("specs() 必须能序列化");
    let with_empty_specs_bytes =
        serde_json::to_vec(with_empty.specs()).expect("specs() 必须能序列化");
    assert_eq!(
        base_specs_bytes, with_empty_specs_bytes,
        "空切片之后，specs() 的字节必须与不调用时逐字节相同"
    );

    for name in ["srv:fs/read", "host-prefix-never-declared"] {
        assert_eq!(
            base.declares(name),
            with_empty.declares(name),
            "declares({name}) 在空切片前后必须一致"
        );
    }

    assert_eq!(
        base.timed(CallTiming::SessionStart).count(),
        with_empty.timed(CallTiming::SessionStart).count(),
        "空切片不该往 timed(SessionStart) 区添一条"
    );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &with_empty).expect("builtin() 没有会失败的开局工具");
    assert!(
        session.prefix_chunks().is_empty(),
        "builtin() 本身没有 SessionStart 工具，空切片的 with_host_prefix 更不该凭空产块"
    );
}

/// 验收 3：与内置 timed（`with_skills` 的索引）共存——内置块必须排在声明块
/// **之前**，即便声明的 name（`aaa-early`）字母序排在 `srv:skill/index` 前面。
/// 用这个 name 是故意的：一个「把整个前缀区按 label 全局重排」的错误实现会
/// 让 `init:aaa-early` 跑到 `init:srv:skill/index` 前面；正确实现只在声明块
/// 内部排序，块与块之间按注册序（`with_skills` 先、`with_host_prefix` 后，
/// 155 §做什么 第 3 条的「表尾原则」）。
#[test]
fn declared_chunks_are_appended_after_the_builtin_timed_chunk_not_globally_resorted() {
    let registry = SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("coexist-flow"),
        description: Arc::from("与 host_prefix 共存测试用的流程"),
        body: Arc::from("正文不该出现在任何索引/前缀块里。"),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }]);
    let declared = pairs(&[("aaa-early", "Early 文本")]);
    let table = ToolTable::builtin()
        .with_skills(registry)
        .with_host_prefix(&declared);

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("内置索引与声明块都该成功");

    let chunks = session.prefix_chunks();
    assert_eq!(chunks.len(), 2, "该恰两块：内置索引 + 一条声明: {chunks:?}");
    assert_eq!(
        &*chunks[0].label, "init:srv:skill/index",
        "内置 timed（skills 索引）该排第一——即便声明的 name 字母序更靠前"
    );
    assert_eq!(&*chunks[1].label, "init:aaa-early", "声明块该排在内置块之后");
    assert_eq!(&*chunks[1].text, "Early 文本", "声明块的文本该原样落地");
}

/// 验收 4（integration 层面）：spawn 点名一个声明块的名字该成功（子请求体
/// system 段含该块文本）；同一棵树上，另起一轮点一个不存在的名字该拒
/// （`is_error` 的 tool_result、树上不再长子）。手法照抄
/// `inherit_prefix_indep.rs`/`inherit_prefix_rejects_indep.rs`：单个 recording
/// server 按顺序应答两轮，`begin_turn` 隔开。
#[test]
fn spawn_inherit_prefix_accepts_a_declared_name_and_rejects_an_unknown_one() {
    let dir = support::temp_dir("host-prefix-inherit");
    const MARKER: &str = "HOST-PREFIX-OPS-NOTES-MARKER-7f2c";
    const ILLEGAL: &str = "ops-notes-does-not-exist";
    let declared = pairs(&[("ops-notes", MARKER)]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_host_prefix(&declared);

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("声明块该成功");

    let wire = wire_name::to_wire("srv:agent/spawn");
    let ok_input = spawn_input(json!({
        "task": "spawn naming the declared host-prefix block",
        "inherit_prefix": ["ops-notes"],
    }));
    let bad_input = spawn_input(json!({
        "task": "spawn naming a name that was never declared",
        "inherit_prefix": [ILLEGAL],
    }));

    let (port, bodies) = support::spawn_recording_server(vec![
        // 轮 1：点名声明块——该成功。父首跳（吐出 spawn 的 tool_call）、子首跳、父收尾跳。
        support::sse_tool_call("call_ok", &wire, &ok_input),
        support::sse_text("child reported"),
        support::sse_text("root received the child"),
        // 轮 2：点一个不存在的名字——该拒，不长子。父首跳 + 收到 is_error 后的收尾跳。
        support::sse_tool_call("call_bad", &wire, &bad_input),
        support::sse_text("root saw the refusal and moved on"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    let status = run_turn(&mut session, &mut ctx, "spawn naming ops-notes")
        .expect("合法名字不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    {
        let bodies = bodies.lock().unwrap();
        let child_system = wire_system_text(&bodies[1]);
        assert!(
            child_system.contains(MARKER),
            "点名声明块之后，子请求体 system 段该含标记文本: {child_system}"
        );
    }

    session.begin_turn();
    let status = run_turn(&mut session, &mut ctx, "spawn naming an unknown name")
        .expect("非法名字该走 is_error 的 tool_result，不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    assert_eq!(
        session.live_agents().len(),
        2,
        "root + 轮一成功的那一个子——非法名字的轮二不该再长一个子"
    );
    assert!(
        session.live_agents().contains(&AgentId::root().child(1)),
        "轮一该成功长出 child(1)"
    );
    assert!(
        !session.live_agents().contains(&AgentId::root().child(2)),
        "轮二的非法名字不该长出 child(2)"
    );

    let results = tool_results(&session);
    let last = results.last().expect("该有 tool_result");
    assert!(last.1, "非法名字该落 is_error: {results:#?}");
    assert!(
        last.0.contains(ILLEGAL),
        "错误文案该点名非法项，才谈得上「可辨认」: {}",
        last.0
    );
}

/// 验收 5：合成条目不进模型面——`specs()`/`declares()` 里没有声明的名字
/// （timed 区既有语义，133 已经钉过「timed 工具不出现在 specs 里」；这里单独
/// 钉住 `with_host_prefix` 合成的这批不例外）。
#[test]
fn synthesized_entries_never_reach_specs_or_declares() {
    let declared = pairs(&[("hidden-a", "A"), ("hidden-b", "B")]);
    let table = ToolTable::builtin().with_host_prefix(&declared);

    for name in ["hidden-a", "hidden-b"] {
        assert!(
            !table.declares(name),
            "合成条目 {name} 不该被 declares() 认作模型可调的工具"
        );
        assert!(
            !table.specs().iter().any(|s| &*s.name == name),
            "合成条目 {name} 不该出现在 specs() 里: {:?}",
            table.specs()
        );
    }
}
