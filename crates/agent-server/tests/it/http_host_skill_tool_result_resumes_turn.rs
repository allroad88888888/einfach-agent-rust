//! Host skill 携带的远端工具，在一个**恢复出来的老会话**里还能不能被执行（141）。
//!
//! # 141：`active_host_tool_request` 已删——这份测试原本证的是它，现在反过来证它没了
//!
//! 这份测试原本靠模型在同一个会话里现场 `srv:skill/activate`/`deactivate`
//! 摆出「先拒绝、激活后放行、停用后又拒绝」三段式，验证「当前 agent 已激活的
//! host skill 携带的远端工具」有一条专门的 dispatch 解析路径
//! （`ToolTable::active_host_tool_request`）。139 先把 `with_skills` 换成
//! read/index 装配，新会话的表里不再有 activate/deactivate 这两个名字；141 把
//! 那条解析路径本身也删了——决策 27 之后 `capabilities.skills[].tools` 非空
//! 在声明这一步就整份 400（140），skill 携带可执行远端工具在 v1 没有任何时机
//! 能生效，「已激活的 skill 授权它携带的远端工具执行」这条机制因此整个没有
//! 存在的理由。
//!
//! 于是这份测试现在验的是反过来那件事：**一个 M13 期真声明过、真激活过带远端
//! 工具的 host skill 的老会话，恢复到今天的代码后，那个远端工具名不再能被
//! 执行**——不管它在老数据里是「仍处于激活状态」还是「已经停用」，dispatch 都
//! 只剩下「表里没有这个名字」这一条路，跟模型编造一个不存在的工具名走的是
//! 同一条 `unknown_tool`（`is_error`，不 panic、不挂起）。这是「状态在、没人
//! 读」在**执行侧**的落点，跟 `docs/issues/141-remove-activation-subsystem.md`
//! §做什么 第 1 条要的「如实的行为变化」是同一件事的另一半：前一半是「正文
//! 不再注入 system」，这一半是「携带的工具也不再能被派发」。
//!
//! 写盘走的是 `agent_runtime::persist::sync` 这条生产代码本身用的路径（跟
//! `capabilities::record` 内部调用的是同一个函数），不是手拼 JSONL 字节，
//! 所以 server 恢复时读到的是一份形状完全真实的老历史。

use crate::support;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentId, AgentValue, AtomKey, HostSkill, Notice, Reversibility, Session, SkillId, Slot};
use agent_server::{Frame, SessionEvent};
use agent_store::Snapshot;
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

/// 一个只带一个远端工具的 host skill 声明——`tools` 非空这个形状本身在今天的
/// `capabilities` 协议校验层已经拒绝（140），这里绕过 HTTP 直接摆老数据，模拟
/// 决策 27 之前的一次真实声明。
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

/// 直接把「已经声明 + `SKILL` 仍激活」的会话状态写进 `{dir}/{id}.jsonl`——不经
/// `RunnerCtx`/`persist::sync`（那条路要用的 `activate_skill`/`deactivate_skill`
/// 已随 141 删除），改为**直接落一张快照**：`Session::primitives()` 拿到声明
/// 之后的正确编码，只手改 `Slot::SkillsActive` 这一项（`value::str_set` 的编码
/// 形状——排序去重的字符串数组，跟 `tool_table_skill_assembly_tests.rs`/
/// `host_skills_indep_restore.rs` 的老数据兼容测试同一个手法），再用
/// `agent_store::SessionStore::snapshot`（`agent_runtime::open_backend` 返回的
/// 后端本身就实现这个 trait）整份落盘。这就是「老数据」的忠实模拟：一份 M13 期
/// 声明过、激活过的会话，从来不需要真的跑过 `RunnerCtx`。
///
/// `RETIRED_SKILL` **不进激活集**（对应老数据里「先激活又被停用」的最终状态——
/// 停用是幂等的终态，不需要在快照里额外体现「曾经激活过又退出」这段历史）。
fn seed_recovered_session(dir: &Path, id: &str) {
    let path = dir.join(format!("{id}.jsonl"));
    let store = agent_runtime::open_backend(Some(path), |_| {});

    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    session.declare_host_skills(vec![
        skill(SKILL, SOURCE_TOOL),
        skill(RETIRED_SKILL, RETIRED_TOOL),
    ]);

    let active_key = AtomKey::Agent(root.clone(), Slot::SkillsActive);
    let mut values = session.primitives();
    for (key, value) in values.iter_mut() {
        if *key == active_key {
            *value = AgentValue::Json(Arc::new(json!([SKILL])));
        }
    }

    store.snapshot(&Snapshot { values });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_sessions_previously_active_host_skill_tool_is_now_an_unknown_tool() {
    let sessions_dir = support::temp_dir("host-skill-dispatch");
    seed_recovered_session(&sessions_dir, CHAT_ID);

    let upstream = FakeServer::start(vec![
        Script::Immediate(tool_reply(
            "call_source",
            "web_3Adiagnostic_2Fread",
            json!({ "path": "src/lib.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("未知工具已被拒绝。")),
        Script::Immediate(tool_reply(
            "call_retired",
            "web_3Aretired_2Fread",
            json!({ "path": "old.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("停用的技能同样被拒绝。")),
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

    // ── source-diagnostics 在老数据里从恢复的那一刻起就是激活状态，但 141 之后
    //    这不再意味着任何东西：它携带的远端工具没有任何解析路径了，走跟凭空
    //    编造的工具名同一条 `unknown_tool`——立刻回 `is_error`，不挂起、不等待
    //    宿主、`pending_tools` 里不会出现它。
    input(&server, "读取 src/lib.rs");
    let after_source = drain_until_terminal(&mut sse);
    assert!(after_source.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == SOURCE_TOOL
    )));
    assert_no_pending(&server);

    // ── retired-diagnostics 在老数据里先激活又被停用：结果跟上面一样——两者
    //    今天没有任何行为差异，这正是「携带的工具也不再能被派发」这条结论的
    //    落点：dispatch 已经不区分「曾经激活」和「从没声明过」。
    input(&server, "尝试读取已停用技能的文件");
    let after_retired = drain_until_terminal(&mut sse);
    assert!(after_retired.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == RETIRED_TOOL
    )));
    assert_no_pending(&server);

    assert_eq!(
        upstream.request_count(),
        4,
        "两段对话各自的『工具调用 + 拒绝之后续跑一跳』必须完整闭环"
    );
}
