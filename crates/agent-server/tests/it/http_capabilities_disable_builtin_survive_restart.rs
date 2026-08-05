//! 076 §验收「恢复原模原样」与「已有历史再带 → 400 `session_has_history`」，
//! **端到端、跨进程形态**（形状照 073 的 `http_capabilities_survive_restart.rs`）。
//!
//! 用户拍板的那条原则对**减法**一视同仁：
//!
//! > 历史对话记录，不用对工具再注入一次。**历史对话就该跟历史一致，原模原样 100% 复刻。**
//!
//! 开关**也进 store**（`Slot::DisabledBuiltins`，journaled），而且这一路的失败症状比
//! 073/064 那两路更隐蔽：声明丢了会「少几个工具」（模型会说「我没有这个工具」），
//! 开关丢了会**多几个工具**——什么症状都没有，直到模型真的去调了那件本该被藏起来的
//! 东西。

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_core::AgentLimits;
use agent_server::{AgentServer, ServerConfig, SessionTemplate, SessionsHandle, ToolTableSpec};
use serde_json::{Value, json};

use support::http_client;
use support::server::{FakeServer, Script};

const CHAT_ID: &str = "caps-disable-restart";

fn switch() -> Value {
    json!({ "disable_builtin": ["srv:agent/spawn", "srv:shell/exec"] })
}

/// **本条的全部意义**：带开关建会话 → 对话一轮 → 关掉 → 同 chatid 重开、**不带任何
/// `capabilities`** → 被关掉的那些**仍然是关掉的**，而且工具表**逐字节与当初相同**
/// （红线 11 的真意——不是「那个工具还是没有」，是「一个字节都没变」，前缀缓存才
/// 接得上）。
///
/// 「逐字节」那一半是这条真正贵的地方：一个「恢复时按今天的部署重建」的实现会让表
/// 多出两件，前缀第一轮就全断——而**功能一切正常**，只是每一轮都全价，外加模型突然
/// 多出两件历史里从没铺垫过的能力。
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_session_keeps_its_switch_without_being_told_again() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-disable-restart");

    let (first_addr, first_sessions) = start(persistent_template(upstream.endpoint(), sessions_dir.clone())).await;
    let created = create(first_addr, json!({ "id": CHAT_ID, "capabilities": switch() }));
    assert_eq!(created.status, 201, "{}", created.body);
    let before = one_turn(&upstream, first_addr).await;
    assert_eq!(
        names(&before),
        vec!["srv:fs/read", "srv:fs/list", "srv:agent/status", "srv:agent/collect"],
        "夹具前提：第一次那一轮，关掉的两件确实不在"
    );
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    // ── 第二次：**请求体里一个 `capabilities` 字都没有**。
    let (second_addr, second_sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;
    let recovered = create(second_addr, json!({ "id": CHAT_ID }));
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(support::extract_json_string_field(&recovered.body, "outcome"), "recovered");
    let after = one_turn(&upstream, second_addr).await;

    assert!(
        !names(&after).contains(&"srv:agent/spawn".to_string()),
        "恢复出来的会话该带回它自己当初那份**减过的**表——开关没落盘的话这里会凭空多出两件：{:?}",
        names(&after)
    );
    assert_eq!(
        tools_bytes(&after),
        tools_bytes(&before),
        "恢复后第一轮的工具表必须与关闭前那一轮逐字节相同，否则恢复出来的会话第一轮就前缀全断（红线 11）"
    );

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// **已有历史的会话再带这个字段 → 400 `session_has_history`**——跟 073 完全同一条闸，
/// 不是新错误码。
///
/// 两个正对照防止这条测了个寂寞：
/// - **只带 `disable_builtin`**（不带 `tools`/`skills`）也照样被拒——`capabilities` 是
///   整体判断的，073 那道闸不能只挡加法那一半；
/// - **不带 `capabilities`** 的同一次重开是 200——被拒的原因是「带了」，不是「这个
///   chatid 有历史就一律不给开」。
#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_history_refuses_a_new_switch() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-disable-history");

    let (addr, sessions) = start(persistent_template(upstream.endpoint(), sessions_dir.clone())).await;
    assert_eq!(create(addr, json!({ "id": CHAT_ID, "capabilities": switch() })).status, 201);
    one_turn(&upstream, addr).await;
    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    let (second_addr, second_sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;

    let refused = create(second_addr, json!({ "id": CHAT_ID, "capabilities": switch() }));
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert_eq!(
        support::extract_json_string_field(&refused.body, "code"),
        "session_has_history",
        "必须是 073 那条可判别错误码（去掉 capabilities 重发），不是 `bad_request`（改名字重发）：{}",
        refused.body
    );

    // 正对照：不带 `capabilities` 的同一次重开是 200——拒的是「带了」，不是「有历史」。
    let ok = create(second_addr, json!({ "id": CHAT_ID }));
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert_eq!(support::extract_json_string_field(&ok.body, "outcome"), "recovered");

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

async fn start(template: SessionTemplate) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(ServerConfig::new(template));
    let sessions = server.sessions();
    let bound = server.bind("127.0.0.1:0".parse().unwrap()).await.expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

/// 落盘 + 开满档（本 issue 要关的正是这一档里的东西）。
fn persistent_template(endpoint: String, sessions_dir: PathBuf) -> SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.tools = ToolTableSpec::Full { spawn_limits: AgentLimits::default() };
    template.default_sessions_dir = Some(sessions_dir);
    template
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

fn input(addr: SocketAddr) {
    let response = http_client::request(addr, "POST", &format!("/sessions/{CHAT_ID}/input"), Some(r#"{"text":"你好"}"#));
    assert_eq!(response.status, 202, "{}", response.body);
}

async fn one_turn(upstream: &FakeServer, addr: SocketAddr) -> Value {
    let before = upstream.request_count();
    input(addr);
    let deadline = Instant::now() + Duration::from_secs(5);
    while upstream.request_count() < before + 1 {
        assert!(Instant::now() < deadline, "等 provider 调用超时，实际 {}", upstream.request_count());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let body = upstream.bodies().swap_remove(before);
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{body}"))
}

fn names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| agent_providers::wire_name::from_wire(t["function"]["name"].as_str().unwrap_or_default()).to_string())
        .collect()
}

/// 整个 `tools` 段的文本——「逐字节相同」比的是这一段，不是工具个数（一个「少了一件、
/// 又多了一件」的实现个数照样对得上）。
fn tools_bytes(body: &Value) -> String {
    serde_json::to_string(&body["tools"]).expect("tools 段该可序列化")
}
