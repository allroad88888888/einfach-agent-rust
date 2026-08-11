//! Host skill 的延迟工具只在当前 agent 激活期间可调度，并复用既有远端工具闭环。
//!
//! # 139 更新：不再靠模型驱动 activate/deactivate
//!
//! 这份测试原本靠模型在同一个会话里现场 `srv:skill/activate`/`deactivate`
//! 摆出「先拒绝、激活后放行、停用后又拒绝」三段式。`with_skills` 换成
//! read/index 装配之后，新会话的表里不再有这两个名字，模型没有工具调用能做
//! 这件事了（`agent_runtime::ToolTable::skill_injection` 那条注入通路本身没删，
//! 141 之前的兼容态，只是新会话走不到）。
//!
//! 于是这份测试改成**直接把「已经激活/已经停用」的会话状态写进磁盘**，让 server
//! 按老会话恢复的路径把它读回来——跟 `Slot::SkillsActive` 是纯状态、
//! `active_host_tool_request` 只看这份状态不看「怎么变成这样的」同一个道理
//! （见 `tool_table_skill_assembly_tests.rs` 的「老会话兼容」测试）。写盘走的是
//! `agent_runtime::persist::sync` 这条生产代码本身用的路径，不是手拼 JSONL 字节。
//!
//! 两个 skill 各代表原测试的一段：`source-diagnostics` 从一开始就是激活状态
//! （验「激活期间可调度」+ pending/reconnect 闭环）；`retired-diagnostics`
//! 先激活又被停用（验「停用之后拒绝」），两者共用同一条真实的远端工具派发/
//! 拒绝逻辑，跟原测试断的是同一件事。

use crate::support;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentId, HostSkill, Location, Notice, Reversibility, Session, SessionConfig, SkillId};
use agent_server::{Frame, SessionEvent};
use agent_tools::ToolExecutor;
use agent_transport::Client;
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CHAT_ID: &str = "host-skill-dispatch";
const SKILL: &str = "source-diagnostics";
const SOURCE_TOOL: &str = "web:diagnostic/read";
const RETIRED_SKILL: &str = "retired-diagnostics";
const RETIRED_TOOL: &str = "web:retired/read";

fn tool_reply(call_id: &str, wire_name: &str, arguments: Value) -> String {
    let call = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": { "name": wire_name, "arguments": arguments.to_string() }
                }]
            },
            "finish_reason": Value::Null
        }]
    });
    let finish = json!({
        "choices": [{ "index": 0, "delta": { "content": "" }, "finish_reason": "tool_calls" }]
    });
    format!("data: {call}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> (u64, Frame) {
    let raw = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    let id = raw.id.expect("服务端每帧都应有游标");
    let frame =
        serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data));
    (id, frame)
}

fn drain_until_terminal(sse: &mut http_client::SseReader) -> Vec<Frame> {
    let mut frames = Vec::new();
    loop {
        let (_, frame) = next_frame(sse);
        let terminal = matches!(
            &frame.event,
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
        );
        frames.push(frame);
        if terminal {
            return frames;
        }
    }
}

fn input(server: &support::http_server::TestServer, text: &str) {
    let body = json!({ "text": text }).to_string();
    let response = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(&body),
    );
    assert_eq!(response.status, 202, "{}", response.body);
}

fn pending(server: &support::http_server::TestServer) -> Value {
    let response = http_client::request(
        server.addr,
        "GET",
        &format!("/sessions/{CHAT_ID}/pending_tools"),
        None,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("pending_tools 应返回 JSON")
}

fn assert_no_pending(server: &support::http_server::TestServer) {
    assert_eq!(pending(server)["pending"], json!([]));
}

/// 一个只带一个远端工具的 host skill 声明。
fn skill(id: &str, tool: &str) -> HostSkill {
    HostSkill {
        id: SkillId::new(id),
        description: Arc::from("按需诊断源码"),
        body: Arc::from("只在激活期间使用源码工具。"),
        tools: vec![agent_core::ToolSpec {
            name: Arc::from(tool),
            description: Arc::from("读取项目源码文件"),
            schema: Arc::new(json!({ "type": "object", "properties": { "path": { "type": "string" } } })),
        }],
        tool_reversibility: [(Arc::from(tool), Reversibility::Pure)].into_iter().collect(),
    }
}

/// 直接把「已经声明 + 已经激活/停用」的会话状态写进 `{dir}/{id}.jsonl`——走的是
/// `agent_runtime::persist::sync` 这条生产代码本身用来落盘的路径（跟
/// `capabilities::record` 内部调用的是同一个函数），不是手拼 JSONL 字节，所以
/// server 恢复时读到的是一份形状完全真实的历史。
fn seed_recovered_session(dir: &Path, id: &str) {
    let path = dir.join(format!("{id}.jsonl"));
    let store = agent_runtime::open_backend(Some(path), |_| {});
    let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
    let mut ctx = agent_runtime::RunnerCtx::new(
        Arc::new(agent_providers::deepseek::DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "fake-key".to_string(),
        fs,
        agent_runtime::ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("seed"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        store,
        Box::new(|_| {}),
    );
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    session.declare_host_skills(vec![
        skill(SKILL, SOURCE_TOOL),
        skill(RETIRED_SKILL, RETIRED_TOOL),
    ]);
    session.begin_turn();
    session.activate_skill(&root, SkillId::new(SKILL)).unwrap();
    session.activate_skill(&root, SkillId::new(RETIRED_SKILL)).unwrap();
    session.begin_turn();
    session
        .deactivate_skill(&root, SkillId::new(RETIRED_SKILL))
        .unwrap();
    agent_runtime::persist::sync(&mut ctx, &mut session);
}

#[tokio::test(flavor = "multi_thread")]
async fn active_host_skill_tool_uses_pending_result_flow_and_deactivation_revokes_it() {
    let sessions_dir = support::temp_dir("host-skill-dispatch");
    seed_recovered_session(&sessions_dir, CHAT_ID);

    let upstream = FakeServer::start(vec![
        Script::Immediate(tool_reply(
            "call_source",
            "web_3Adiagnostic_2Fread",
            json!({ "path": "src/lib.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("源码读取结果已收到。")),
        Script::Immediate(tool_reply(
            "call_retired",
            "web_3Aretired_2Fread",
            json!({ "path": "old.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("停用的技能已被拒绝。")),
    ]);
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.default_sessions_dir = Some(sessions_dir);
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |config| {
            config
                .with_ring_capacity(32)
                .with_cancel_grace(Duration::from_secs(2))
        },
    )
    .await;

    // ── 不带 capabilities：这个 chatid 已经有历史了，声明只能从历史来
    //    （073 的既有闸，`http_capabilities_skills_survive_restart.rs` 同款）。
    let created = http_client::request(server.addr, "POST", "/sessions", Some(&json!({ "id": CHAT_ID }).to_string()));
    assert_eq!(created.status, 200, "{}", created.body);

    let (status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{CHAT_ID}/events"), None);
    assert_eq!(status, 200);

    // ── source-diagnostics 从恢复的那一刻起就是激活状态：模型直接调它的远端
    //    工具，走既有的 pending/reconnect/tool_result 闭环。
    input(&server, "读取 src/lib.rs");
    let (executing_id, agent, call_id) = loop {
        let (id, frame) = next_frame(&mut sse);
        if let SessionEvent::ToolExecuting { call_id, request } = frame.event {
            assert_eq!(request.location, Location::Web);
            assert_eq!(request.reversibility, Reversibility::Pure);
            assert_eq!(*request.input, json!({ "path": "src/lib.rs" }));
            break (id, frame.agent, call_id);
        }
    };

    drop(sse);
    let waiting = pending(&server);
    let items = waiting["pending"].as_array().expect("pending 应为数组");
    assert_eq!(items.len(), 1, "断线后必须仍能恢复唯一待办：{waiting}");
    assert_eq!(items[0]["agent"], serde_json::to_value(&agent).unwrap());
    assert_eq!(items[0]["call_id"], serde_json::to_value(&call_id).unwrap());
    assert_eq!(items[0]["request"]["tool"], SOURCE_TOOL);
    assert_eq!(items[0]["request"]["location"], "Web");
    assert_eq!(items[0]["request"]["reversibility"], "Pure");

    let (status, _, mut sse) = http_client::connect_sse(
        server.addr,
        &format!("/sessions/{CHAT_ID}/events"),
        Some(executing_id),
    );
    assert_eq!(status, 200);
    let result = json!({
        "agent": agent,
        "tool_call_id": call_id,
        "result": { "content": "pub fn answer() -> u8 { 42 }" }
    });
    let response = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/tool_result"),
        Some(&result.to_string()),
    );
    assert_eq!(response.status, 202, "{}", response.body);
    let resumed = drain_until_terminal(&mut sse);
    assert!(resumed.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == SOURCE_TOOL
    )));
    assert_no_pending(&server);

    // ── retired-diagnostics 在恢复的历史里先激活又被停用：它的远端工具现在
    //    该被当成未声明的工具直接拒绝（不挂起、不等待宿主）。
    input(&server, "尝试读取已停用技能的文件");
    let after = drain_until_terminal(&mut sse);
    assert!(after.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == RETIRED_TOOL
    )));
    assert_no_pending(&server);

    assert_eq!(
        upstream.request_count(),
        4,
        "两段对话（一次远端派发 + 一次就地拒绝）的 provider 跳数必须完整闭环"
    );
}
