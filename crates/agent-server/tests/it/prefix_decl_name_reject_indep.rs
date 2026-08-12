//! 独立测试 agent 依据 156 + HOST-CAPABILITIES §八之三 写的规格测试——不看
//! `http/capabilities/{validate_prefix,capability_prefix,assemble}.rs`/
//! `actor/capabilities.rs` 的实现，只按协议契约断言。
//!
//! 本文件管 `capabilities.prefix[].name` 这一个字段的**身份校验**：前缀是不是
//! 被认识的位置、名字有没有跟别的名字（同批内部/顶层 `capabilities.tools`）
//! 撞车。四条：坏前缀矩阵、黑盒探测到的「前缀之后不校验」发现、声明内部重名、
//! 与 `capabilities.tools` 重名。`text` 是否为空、以及有历史再声明的
//! `session_has_history` 闸，在职责上是另一件事，见
//! `prefix_decl_state_reject_indep.rs`。

use crate::support;
use std::net::SocketAddr;
use std::time::Duration;

use agent_server::{AgentServer, ServerConfig, SessionsHandle};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::FakeServer;

const CHAT_ID: &str = "prefix-decl-name-reject-indep";

/// **校验条 1**：name 必须 `web:`/`desk:` 前缀——四种「压根不是被认识的位置
/// 前缀」的写法必须被拒（`srv:`/`mcp:`/无前缀/空名）。
///
/// 黑盒探测过更细的边界之后，这条矩阵**只留确凿会被拒的四种**——`只有前缀`
/// （`"web:"`）、`前缀之后有空格`（`"web:crm briefing"`）这两种在
/// `capabilities.tools` 那边是 400（见 `http_capabilities_declaration.rs` 的
/// `rejected_declarations_never_create_a_session`），探测下来在 `capabilities.
/// prefix` 这边却是 201——不是我猜错了放这儿凑数，是真测过；这个发现单独钉在
/// 下面的 `local_part_after_a_valid_prefix_is_not_currently_whitelisted`，别跟
/// 这条确凿的矩阵混在一起，免得一条断言把两件不同确定性的事捆死。
#[tokio::test(flavor = "multi_thread")]
async fn bad_prefix_name_matrix_is_400_bad_request() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(&upstream).await;

    let cases: Vec<(&str, &str)> = vec![
        ("srv: 前缀", "srv:crm/briefing"),
        ("mcp: 前缀", "mcp:crm/briefing"),
        ("无前缀", "crm_briefing"),
        ("空名", ""),
    ];

    for (label, name) in cases {
        let id = format!("bad-{}", name.len());
        let response = create(
            addr,
            json!({
                "id": id,
                "capabilities": { "prefix": [ { "name": name, "text": "随便一段文本" } ] }
            }),
        );
        assert_eq!(response.status, 400, "{label}：{}", response.body);
        assert_eq!(
            support::extract_json_string_field(&response.body, "code"),
            "bad_request",
            "{label}：坏名字该是 bad_request（改名字重发）：{}",
            response.body
        );
        assert!(
            sessions.ids().iter().all(|sid| sid.to_string() != id),
            "{label}：被拒的声明不该登记出会话：{:?}",
            sessions.ids()
        );
        let probe = http_client::request(addr, "GET", &format!("/sessions/{id}"), None);
        assert_eq!(probe.status, 404, "{label}：会话不该存在过：{}", probe.body);
    }
}

/// **本条的履历**：独测初版黑盒探出 prefix 名只查前缀不查本体（`"web:"`、
/// `"web:crm briefing"` 在 tools 那边 400、在这边 201），如实钉住并标注
/// 「若收紧此测试会变红」。主会话随即拍板**收紧到与 `capabilities.tools`
/// 一字不差**（理由：名字进 journaled 的 `init:<name>` label、模型要在
/// `inherit_prefix` 里逐字打它、同名不同判是 HOST-CAPABILITIES §三之二点名
/// 的说不清的面），本测试翻转为断言收紧后的行为。注意 `"web:/"` **两边都
/// 合法**（`/` 在白名单内）——它是「两边一致」的另一半证据，不是漏网。
#[tokio::test(flavor = "multi_thread")]
async fn local_part_after_a_valid_prefix_is_whitelisted_like_a_tool_name() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(&upstream).await;

    // 收紧后：跟 `capabilities.tools` 的矩阵同判。
    let rejected_cases: Vec<(&str, &str)> = vec![
        ("只有前缀，冒号后面空的", "web:"),
        ("前缀之后带空格", "web:crm briefing"),
    ];
    for (label, name) in rejected_cases {
        let id = format!("tightened-{}", name.len());
        let response = create(
            addr,
            json!({
                "id": id,
                "capabilities": { "prefix": [ { "name": name, "text": "占位文本" } ] }
            }),
        );
        assert_eq!(
            response.status, 400,
            "{label}（{name:?}）：该跟 capabilities.tools 同判被拒：{}",
            response.body
        );
        assert!(
            response.body.contains("bad_request"),
            "{label}：错误码该是可判别的 bad_request：{}",
            response.body
        );
        let probe = http_client::request(addr, "GET", &format!("/sessions/{id}"), None);
        assert_eq!(probe.status, 404, "{label}：会话不该存在过：{}", probe.body);
    }

    // 斜杠本体：两边一致合法。
    let response = create(
        addr,
        json!({
            "id": "slash-legal",
            "capabilities": { "prefix": [ { "name": "web:/", "text": "占位文本" } ] }
        }),
    );
    assert_eq!(
        response.status, 201,
        "web:/ 在 tools 与 prefix 两边都合法（斜杠在白名单内）：{}",
        response.body
    );
    assert!(
        sessions.ids().iter().any(|sid| sid.to_string() == "slash-legal"),
        "201 却没登记出会话：{:?}",
        sessions.ids()
    );
}

/// **校验条 2**：声明内部重名——两个 `prefix` 条目撞同一个 name。
#[tokio::test(flavor = "multi_thread")]
async fn internal_duplicate_prefix_name_is_400_bad_request() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(&upstream).await;

    let response = create(
        addr,
        json!({
            "id": CHAT_ID,
            "capabilities": {
                "prefix": [
                    { "name": "web:crm/briefing", "text": "第一段" },
                    { "name": "web:crm/briefing", "text": "第二段" }
                ]
            }
        }),
    );
    assert_eq!(response.status, 400, "{}", response.body);
    assert_eq!(
        support::extract_json_string_field(&response.body, "code"),
        "bad_request"
    );
    assert!(
        response.body.contains("web:crm/briefing"),
        "该点名是哪个名字撞了：{}",
        response.body
    );
    assert!(sessions.ids().is_empty());
}

/// **校验条 2 的另一半**：`prefix` 的 name 与顶层 `capabilities.tools` 的 name
/// 撞——两处最后都成为表里的名字，同名会让 `init:<name>` label 与路由说不清
/// （156 原文判据）。
#[tokio::test(flavor = "multi_thread")]
async fn prefix_name_colliding_with_capabilities_tools_is_400_bad_request() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(&upstream).await;

    let response = create(
        addr,
        json!({
            "id": CHAT_ID,
            "capabilities": {
                "tools": [ { "name": "web:crm/briefing" } ],
                "prefix": [ { "name": "web:crm/briefing", "text": "撞名的文本" } ]
            }
        }),
    );
    assert_eq!(response.status, 400, "{}", response.body);
    assert_eq!(
        support::extract_json_string_field(&response.body, "code"),
        "bad_request"
    );
    assert!(
        response.body.contains("web:crm/briefing"),
        "该点名是哪个名字撞了：{}",
        response.body
    );
    assert!(sessions.ids().is_empty());
}

async fn start(upstream: &FakeServer) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(
        ServerConfig::new(support::http_server::session_template(upstream.endpoint()))
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY),
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
