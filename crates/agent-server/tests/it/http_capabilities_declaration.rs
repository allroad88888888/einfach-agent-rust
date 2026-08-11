//! 061：`POST /sessions` 的 `capabilities` 声明——合法的照常建会话，名字不合规的
//! 一律 400 **且会话根本没被建出来**（不只断状态码：既问 registry 的 `ids()`，
//! 也顺手 `GET /sessions/:id` 看它是不是 404）。

use crate::support;
use std::net::SocketAddr;

use agent_server::{AgentServer, ServerConfig, SessionTemplate, SessionsHandle};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::FakeServer;

const CHAT_ID: &str = "capabilities-chat";

/// 一份「像真的」的声明：两个顶层工具（一个标了 `pure`、一个没标）+ 一个 skill。
/// 061 到此为止——工具表这一刻还没变，那是 062。
///
/// 140 之前这个 skill 还带过一个自带工具（`web:crm/close-ticket`）——决策 27 之后
/// v1 不支持 skill 携带工具，那样的声明会 400，所以从这份「合法声明」的样本里
/// 删掉了；工具想给这个 skill 用，走顶层 `tools`（正是上面那两个）。
fn valid_capabilities() -> Value {
    json!({
        "tools": [
            { "name": "web:crm/lookup",
              "description": "按客户 ID 查 CRM 档案",
              "schema": { "type": "object", "properties": { "id": { "type": "string" } } },
              "reversibility": "pure" },
            { "name": "desk:clipboard/write", "description": "写系统剪贴板" }
        ],
        "skills": [
            { "id": "crm-flow",
              "description": "处理客户工单的标准流程",
              "body": "第一步：查档案。第二步：……" }
        ]
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_valid_declaration_creates_the_session_and_stays_idempotent() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(support::http_server::session_template(upstream.endpoint())).await;

    let created = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": valid_capabilities() }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(
        support::extract_json_string_field(&created.body, "outcome"),
        "created"
    );
    assert!(
        sessions.ids().iter().any(|id| id.to_string() == CHAT_ID),
        "合法声明该真的建出会话：{:?}",
        sessions.ids()
    );

    // 带声明也照旧是幂等 getOrCreate（055 的三态没被 061 改动）。
    let again = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": valid_capabilities() }),
    );
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(
        support::extract_json_string_field(&again.body, "outcome"),
        "existing"
    );
    assert_eq!(sessions.ids().len(), 1, "重复请求不该多建一个会话");

    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

/// 每一条不合规声明：400 + 统一错误形状 + **registry 里没有这个会话**
/// （`sessions.ids()` 全空，且 `GET /sessions/:id` 是 404，不是「存在但死了」）。
#[tokio::test(flavor = "multi_thread")]
async fn rejected_declarations_never_create_a_session() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(support::http_server::session_template(upstream.endpoint())).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "srv: 前缀",
            json!({ "tools": [ { "name": "srv:crm/lookup" } ] }),
        ),
        (
            "mcp: 前缀",
            json!({ "tools": [ { "name": "mcp:everything/echo" } ] }),
        ),
        ("无前缀", json!({ "tools": [ { "name": "crm_lookup" } ] })),
        ("空名", json!({ "tools": [ { "name": "" } ] })),
        ("只有前缀", json!({ "tools": [ { "name": "web:" } ] })),
        (
            "前缀之后有空格",
            json!({ "tools": [ { "name": "web:crm lookup" } ] }),
        ),
        // 140：v1 不支持 skill 携带工具（决策 27）——形状不合法的工具挂在 skill 上，
        // 报的已经不是「前缀不对」，是「skill 根本不该带 tools」；这条判定排在
        // 前缀/形状检查之前，所以这里不管工具名本身合不合法都是同一条 400。
        (
            "skill 自带 srv: 工具（形状也不合法）",
            json!({ "skills": [ { "id": "crm-flow", "tools": [ { "name": "srv:crm/lookup" } ] } ] }),
        ),
        (
            "skill 自带工具（形状合法也不行——v1 不支持）",
            json!({ "skills": [ { "id": "crm-flow", "tools": [ { "name": "web:crm/close" } ] } ] }),
        ),
        (
            "顶层工具撞名",
            json!({ "tools": [ { "name": "web:a/b" }, { "name": "web:a/b" } ] }),
        ),
        (
            // 140 之前这条测的是「跨顶层与 skill 边界的重名」（DuplicateTool）；
            // 现在 skill 一旦带 tools 就先撞 SkillCarriesTools，撞不撞名已经无关
            // 紧要——这条案例保留是为了钉住「先撞哪一条」这件事本身。
            "skill 工具与顶层撞名（现在先撞的是「skill 不许带工具」）",
            json!({ "tools": [ { "name": "web:a/b" } ], "skills": [ { "id": "s1", "tools": [ { "name": "web:a/b" } ] } ] }),
        ),
        (
            "skill id 撞名",
            json!({ "skills": [ { "id": "s1" }, { "id": "s1" } ] }),
        ),
        (
            "skill id 字符集",
            json!({ "skills": [ { "id": "crm/flow" } ] }),
        ),
        ("skill id 为空", json!({ "skills": [ { "id": "" } ] })),
    ];

    for (label, capabilities) in cases {
        let response = create(addr, json!({ "id": CHAT_ID, "capabilities": capabilities }));
        assert_eq!(response.status, 400, "{label}：{}", response.body);
        assert!(
            response.body.contains("\"bad_request\""),
            "{label}：{}",
            response.body
        );
        assert!(
            sessions.ids().is_empty(),
            "{label}：不合规的声明不得登记 session：{:?}",
            sessions.ids()
        );
        let status = http_client::request(addr, "GET", &format!("/sessions/{CHAT_ID}"), None);
        assert_eq!(
            status.status, 404,
            "{label}：会话不该存在过：{}",
            status.body
        );
    }
}

/// 错误文案要能直接回给调用方：说得清是哪一项、为什么。
#[tokio::test(flavor = "multi_thread")]
async fn the_rejection_message_says_which_item_and_why() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(support::http_server::session_template(upstream.endpoint())).await;

    let response = create(
        addr,
        json!({ "capabilities": { "tools": [ { "name": "srv:crm/lookup" } ] } }),
    );
    assert_eq!(response.status, 400, "{}", response.body);
    assert!(
        response.body.contains("srv:crm/lookup"),
        "该指出是哪一项：{}",
        response.body
    );
    assert!(
        response.body.contains("capabilities.tools"),
        "该指出在哪儿声明的：{}",
        response.body
    );
    assert!(
        response.body.contains("web:"),
        "该说清合法前缀是什么：{}",
        response.body
    );

    // 140：这份声明触发的已经不是「工具形状」，是「skill 根本不该带 tools」——
    // 消息要点名是哪个 skill、指向裁剪依据（决策 27）、说清正确的声明位置。
    let response = create(
        addr,
        json!({ "capabilities": { "skills": [ { "id": "s1", "tools": [ { "name": "srv:x/y" } ] } ] } }),
    );
    assert_eq!(response.status, 400, "{}", response.body);
    assert!(
        response.body.contains("skill \\\"s1\\\""),
        "该指出是哪个 skill 带的：{}",
        response.body
    );
    assert!(
        response.body.contains("决策 27"),
        "该指向裁剪依据：{}",
        response.body
    );
    assert!(
        response.body.contains("capabilities.tools"),
        "该说清工具该往哪儿声明：{}",
        response.body
    );
    assert!(sessions.ids().is_empty());
}

/// 向后兼容：不带 `capabilities` 的老请求、以及空声明，行为跟 061 之前一致。
#[tokio::test(flavor = "multi_thread")]
async fn omitting_or_emptying_capabilities_keeps_the_old_behavior() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(support::http_server::session_template(upstream.endpoint())).await;

    let legacy = create(addr, json!({}));
    assert_eq!(legacy.status, 201, "{}", legacy.body);
    assert!(
        !legacy.body.contains("\"outcome\""),
        "旧请求的响应形状不变：{}",
        legacy.body
    );

    for body in [
        json!({ "capabilities": {} }),
        json!({ "capabilities": { "tools": [], "skills": [] } }),
    ] {
        let response = create(addr, body.clone());
        assert_eq!(response.status, 201, "{body}：{}", response.body);
    }

    assert_eq!(
        sessions.ids().len(),
        3,
        "三次请求各建一个会话：{:?}",
        sessions.ids()
    );
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

async fn start(template: SessionTemplate) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY),
    );
    let sessions = server.sessions();
    let bound = server
        .bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (addr, sessions)
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}
