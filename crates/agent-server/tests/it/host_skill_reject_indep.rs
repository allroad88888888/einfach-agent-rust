//! 独立测试 agent 依据 140（docs/issues/140-host-skills-into-registry.md）「验收」/
//! 「注意」两节写的规格测试——不看实现，139/140 未落地时红是预期。三条契约：
//! 1) skill 带非空 `tools` → 整份 400，会话不创建（同 id 再建不带历史）；
//! 2) 干净 skill → 首轮 system 含索引行，模型脚本化调 `srv:skill/read` → 正文逐
//! 字节进 tool_result；3) 落盘重启恢复后索引行逐字节不变（红线 11）、read 仍可用。
//!
//! `srv:skill/read` 的参数名不在验收文字里，不臆测：从假上游收到的请求体里现读
//! 它的 JSON Schema（`required` 或退到 `properties` 首个 key）现造参数。

use crate::support;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_server::{AgentServer, ServerConfig, SessionsHandle};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CHAT_ID: &str = "host-skill-reject-indep";
const SKILL_ID: &str = "diag-notes";
const SKILL_DESC: &str = "诊断记录标准流程";
const SKILL_BODY_MARKER: &str = "SKILL_READ_BODY_MARKER_H140_9YQZ";

fn clean_skill_declaration() -> Value {
    json!({
        "skills": [{
            "id": SKILL_ID,
            "description": SKILL_DESC,
            "body": format!("先看日志再定位问题。{SKILL_BODY_MARKER}")
        }]
    })
}

fn skill_with_tools_declaration() -> Value {
    json!({
        "skills": [{
            "id": SKILL_ID,
            "description": SKILL_DESC,
            "body": "先看日志再定位问题。",
            "tools": [{ "name": "web:diagnostic/read", "description": "读取一份诊断记录" }]
        }]
    })
}

/// 契约 1：声明里带 `tools` 的 skill → 整份 400，会话不创建，同 id 再建不带历史。
#[tokio::test(flavor = "multi_thread")]
async fn declaring_a_skill_with_tools_rejects_the_whole_request_and_creates_nothing() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start_full(upstream.endpoint(), None).await;

    let rejected = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": skill_with_tools_declaration() }),
    );
    assert_eq!(rejected.status, 400, "{}", rejected.body);
    assert!(rejected.body.contains("\"bad_request\""), "该是既有统一的 400 错误形状：{}", rejected.body);
    // 决策 27：v1 不支持 skill 自带工具——错误体该指出是哪个 skill 携带工具被拒。
    assert!(rejected.body.contains(SKILL_ID), "{}", rejected.body);
    assert!(sessions.ids().is_empty(), "被拒的声明不该登记出会话：{:?}", sessions.ids());
    let status = http_client::request(addr, "GET", &format!("/sessions/{CHAT_ID}"), None);
    assert_eq!(status.status, 404, "被拒之后会话不该存在过：{}", status.body);

    // 同 id 再建（不带声明）：真没留下就该是全新 201 created，不是带历史的 existing/recovered。
    let retried = create(addr, json!({ "id": CHAT_ID }));
    assert_eq!(retried.status, 201, "{}", retried.body);
    let outcome = support::extract_json_string_field(&retried.body, "outcome");
    assert_eq!(outcome, "created", "同 id 再建不该带着被拒那次的任何痕迹：{}", retried.body);

    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// 契约 2：干净 skill → 首轮 system 段含索引行；模型脚本化调 `srv:skill/read` →
/// 正文逐字节进 tool_result（某条 `role: tool` 消息里）。
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_skill_indexes_then_reads_verbatim_via_tool_result() {
    let upstream = FakeServer::start(vec![
        Script::Dynamic(Arc::new(read_call_from_request)),
        Script::Immediate(support::wire::text_reply("读完了。")),
    ]);
    let addr = start(&upstream).await;

    let created = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": clean_skill_declaration() }),
    );
    assert_eq!(created.status, 201, "{}", created.body);

    input(addr, CHAT_ID);
    wait_for(&upstream, 2).await;

    let first = body_at(&upstream, 0);
    assert!(
        has_index_line(&system_text(&first)),
        "干净 skill 首轮 system 段该含它的索引行（id 与 description）：{}",
        system_text(&first)
    );

    let after_read = body_at(&upstream, 1);
    assert!(
        tool_result_carries(&after_read),
        "read 之后的下一跳该在某条 role=tool 的消息里带上正文逐字节：{after_read}"
    );
}

/// 契约 3：会话落盘后重启恢复 → 首轮 system 索引行与恢复前逐字节相同（红线 11）、
/// read 仍能取到同一份正文（registry 从持久化的 `host_skills` 状态重建）。
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_session_keeps_the_index_byte_identical_and_can_still_read() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(support::wire::text_reply("好的。")), // 关闭前那一轮基线
        Script::Immediate(support::wire::text_reply("好的。")), // 恢复后第一轮基线
        Script::Dynamic(Arc::new(read_call_from_request)),      // 恢复后第二轮：脚本化调 read
        Script::Immediate(support::wire::text_reply("读完了。")),
    ]);
    let sessions_dir = support::temp_dir("host-skill-reject-indep-restart");

    let (first_addr, first_sessions) =
        start_full(upstream.endpoint(), Some(sessions_dir.clone())).await;
    let created = create(
        first_addr,
        json!({ "id": CHAT_ID, "capabilities": clean_skill_declaration() }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let before = one_turn(&upstream, first_addr, CHAT_ID).await;
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    let (second_addr, second_sessions) = start_full(upstream.endpoint(), Some(sessions_dir)).await;
    let recovered = create(second_addr, json!({ "id": CHAT_ID }));
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(
        support::extract_json_string_field(&recovered.body, "outcome"),
        "recovered"
    );
    let after = one_turn(&upstream, second_addr, CHAT_ID).await;

    assert_eq!(
        system_text(&after),
        system_text(&before),
        "恢复后首轮 system 的索引行必须与关闭前逐字节相同（红线 11）"
    );
    assert!(
        has_index_line(&system_text(&after)),
        "恢复出来的会话该还带着这条索引行：{}",
        system_text(&after)
    );

    // 再来一轮脚本化调 read：这次 create 请求体里没有 capabilities，取到正文靠的
    // 只能是持久化的 host_skills 状态。
    let before_read = upstream.request_count();
    input(second_addr, CHAT_ID);
    wait_for(&upstream, before_read + 2).await;
    let after_read = body_at(&upstream, before_read + 1);
    assert!(
        tool_result_carries(&after_read),
        "重启恢复之后 read 仍要能取到同一份正文：{after_read}"
    );

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// system 段里有一行**同时**带着这个 skill 的 id 与 description——140 的验收只说
/// 「索引行（id 与 description）」，没定分隔符字面量，不锁死成某个具体符号。
fn has_index_line(text: &str) -> bool {
    text.lines()
        .any(|line| line.contains(SKILL_ID) && line.contains(SKILL_DESC))
}

/// 某条 `role: tool` 的消息里逐字节带着这个 skill 的正文标记。
fn tool_result_carries(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|m| m["role"] == json!("tool") && m.to_string().contains(SKILL_BODY_MARKER))
}

/// 从请求体里现读 `srv:skill/read` 的 JSON Schema，现造一次调用——不硬编码参数名。
fn read_call_from_request(raw: &str) -> String {
    let body: Value =
        serde_json::from_str(raw).unwrap_or_else(|error| panic!("请求体不是 JSON：{error}\n{raw}"));
    let tools = body["tools"].as_array().cloned().unwrap_or_default();
    let read_tool = tools
        .iter()
        .find(|t| {
            &*agent_providers::wire_name::from_wire(t["function"]["name"].as_str().unwrap_or_default())
                == "srv:skill/read"
        })
        .unwrap_or_else(|| {
            panic!("首轮工具表里没有 srv:skill/read（139/140 未装配时红是预期）：{body}")
        });
    let wire_name = read_tool["function"]["name"].as_str().unwrap_or_default();
    let params = &read_tool["function"]["parameters"];
    let key = params["required"]
        .as_array()
        .and_then(|required| required.first())
        .and_then(Value::as_str)
        .or_else(|| {
            params["properties"]
                .as_object()
                .and_then(|props| props.keys().next())
                .map(String::as_str)
        })
        .unwrap_or("id");
    let mut arguments = serde_json::Map::new();
    arguments.insert(key.to_string(), json!(SKILL_ID));
    tool_call_sse("call_read", wire_name, &Value::Object(arguments))
}

/// 一段 DeepSeek 形状的流式回复：模型调用 `wire_name(arguments)`。
fn tool_call_sse(call_id: &str, wire_name: &str, arguments: &Value) -> String {
    let args = serde_json::to_string(&arguments.to_string()).expect("json string");
    format!(
        concat!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":null,",
            "\"tool_calls\":[{{\"index\":0,\"id\":\"{call_id}\",\"type\":\"function\",",
            "\"function\":{{\"name\":\"{wire_name}\",\"arguments\":{args}}}}}]}}}}]}}\n\n",
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"\"}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        call_id = call_id,
        wire_name = wire_name,
        args = args
    )
}

async fn start(upstream: &FakeServer) -> SocketAddr {
    start_full(upstream.endpoint(), None).await.0
}

async fn start_full(endpoint: String, sessions_dir: Option<PathBuf>) -> (SocketAddr, SessionsHandle) {
    let mut template = support::http_server::session_template(endpoint);
    template.default_sessions_dir = sessions_dir;
    let server = AgentServer::new(
        ServerConfig::new(template).with_private_capability(support::http_server::PRIVATE_CAPABILITY),
    );
    let sessions = server.sessions();
    let bound = server
        .bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

fn input(addr: SocketAddr, id: &str) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some(r#"{"text":"你好"}"#),
    );
    assert_eq!(response.status, 202, "{}", response.body);
}

async fn one_turn(upstream: &FakeServer, addr: SocketAddr, id: &str) -> Value {
    let before = upstream.request_count();
    input(addr, id);
    wait_for(upstream, before + 1).await;
    body_at(upstream, before)
}

async fn wait_for(upstream: &FakeServer, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while upstream.request_count() < want {
        assert!(
            Instant::now() < deadline,
            "等第 {want} 次 provider 调用超时，实际 {}",
            upstream.request_count()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn body_at(upstream: &FakeServer, index: usize) -> Value {
    let body = upstream.bodies().swap_remove(index);
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{body}"))
}

fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .expect("请求体里该有 messages")
        .iter()
        .find(|m| m["role"] == json!("system"))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}
