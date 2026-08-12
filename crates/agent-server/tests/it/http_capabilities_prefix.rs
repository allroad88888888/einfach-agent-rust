//! 156 的核心验收，非重启部分：`capabilities.prefix`（M17，决策 31）从声明到
//! 首轮 encode body 的 system 段——两块都在、排在内置 init 块（skill 索引）之后、
//! 块间按 name 序；不带 `prefix` 的请求体与本条落地前逐字节相同；校验矩阵四项
//! 各 400 且点名。
//!
//! 断言落点跟 062/064/076 一样是**假上游收到的请求体**：料对不对的唯一判据是
//! 模型看到了什么（红线 11），不是内部某个 `Vec` 长什么样。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const PREFIX_A_NAME: &str = "web:crm/briefing";
const PREFIX_A_TEXT: &str = "PREFIX-A-8f21 今天的客户上下文";
const PREFIX_B_NAME: &str = "desk:ops/standup";
const PREFIX_B_TEXT: &str = "PREFIX-B-63dc 今天的运维简报";

/// 一个 skill（带来内置 init 块：`srv:skill/index`）+ 两个开局块，**故意不按
/// 字典序给**（字典序上 `desk:` < `web:`，这里 `web:crm/briefing` 排在数组
/// 前面）：进前缀块要按名字重排，不能原样搬数组顺序，且要排在 skill 索引这个
/// 内置 init 块之后。
fn declaration() -> Value {
    json!({
        "skills": [
            { "id": "crm-flow",
              "description": "处理客户工单的标准流程",
              "body": "第一步：查档案。" }
        ],
        "prefix": [
            { "name": PREFIX_A_NAME, "text": PREFIX_A_TEXT },
            { "name": PREFIX_B_NAME, "text": PREFIX_B_TEXT }
        ]
    })
}

/// 验收第 1 条：两块都在、排在 skill 索引之后、块间按 name 序
/// （`desk:ops/standup` 排在 `web:crm/briefing` 之后——字典序 `d` < `w`，
/// 声明数组反过来给正是为了证明进表按名字重排,不是原样搬）。
#[tokio::test(flavor = "multi_thread")]
async fn two_declared_prefix_blocks_land_after_the_skill_index_in_name_order() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start(&upstream).await;

    create(addr, json!({ "id": "declared", "capabilities": declaration() }));
    let body = one_turn(&upstream, addr, "declared").await;
    let text = system_text(&body);

    let index_pos = text.find("crm-flow — 处理客户工单的标准流程");
    let a_pos = text.find(PREFIX_A_TEXT);
    let b_pos = text.find(PREFIX_B_TEXT);
    assert!(
        index_pos.is_some() && a_pos.is_some() && b_pos.is_some(),
        "skill 索引 + 两个开局块都该在 system 段里：{text}"
    );
    assert!(
        index_pos.unwrap() < a_pos.unwrap() && index_pos.unwrap() < b_pos.unwrap(),
        "两个开局块该排在内置 init 块（skill 索引）之后：{text}"
    );
    assert!(
        b_pos.unwrap() < a_pos.unwrap(),
        "块间该按 name 排序（web:crm/briefing 声明在前，但字典序 desk: < web:，desk:ops/standup 该排在它前面）：{text}"
    );
}

/// 验收第 2 条：不带 `prefix` 字段的请求体与本条落地前逐字节相同——比较手法是
/// 拿同一份 `tools` 声明分别配「不带 prefix」和「prefix: []」两次建会话，两次
/// 首轮请求体（`messages`/`tools` 全量）逐字节相等（红线 11：`[]` 与省略是
/// 同一件事,不能有一条隐藏的「非空才生效」分支导致两者字节不同）。
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_or_omitted_prefix_field_is_byte_identical() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start(&upstream).await;

    let tools_only = json!({ "tools": [ { "name": "web:crm/lookup" } ] });
    create(
        addr,
        json!({ "id": "omitted", "capabilities": tools_only.clone() }),
    );
    let mut with_empty = tools_only.clone();
    with_empty["prefix"] = json!([]);
    create(addr, json!({ "id": "explicit-empty", "capabilities": with_empty }));

    let omitted = one_turn(&upstream, addr, "omitted").await;
    let explicit_empty = one_turn(&upstream, addr, "explicit-empty").await;
    assert_eq!(
        serde_json::to_string(&omitted).unwrap(),
        serde_json::to_string(&explicit_empty).unwrap(),
        "省略 prefix 与 prefix: [] 必须产出逐字节相同的请求体"
    );
}

/// 验收第 3 条：校验矩阵——坏前缀 / 内部重名 / 与 tools 重名 / 空 text，各 400
/// 且点名，且会话根本没被建出来（同 `http_capabilities_declaration.rs` 的手法）。
#[tokio::test(flavor = "multi_thread")]
async fn the_validation_matrix_rejects_each_case_and_names_it() {
    let upstream = FakeServer::start(vec![]);
    let addr = start(&upstream).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "坏前缀",
            json!({ "prefix": [ { "name": "srv:crm/briefing", "text": "t" } ] }),
            "srv:crm/briefing",
        ),
        (
            "内部重名",
            json!({
                "prefix": [
                    { "name": "web:a/b", "text": "第一份" },
                    { "name": "web:a/b", "text": "第二份" }
                ]
            }),
            "web:a/b",
        ),
        (
            "与 tools 重名",
            json!({
                "tools": [ { "name": "web:a/b" } ],
                "prefix": [ { "name": "web:a/b", "text": "t" } ]
            }),
            "web:a/b",
        ),
        (
            "空 text",
            json!({ "prefix": [ { "name": "web:crm/briefing", "text": "" } ] }),
            "web:crm/briefing",
        ),
    ];

    for (label, capabilities, needle) in cases {
        let id = format!("rejected-{}", label.len());
        let response = create_raw(addr, json!({ "id": id, "capabilities": capabilities }));
        assert_eq!(response.status, 400, "{label}：{}", response.body);
        assert!(
            response.body.contains("\"bad_request\""),
            "{label}：{}",
            response.body
        );
        assert!(
            response.body.contains(needle),
            "{label}：错误文案该点名是哪一项：{}",
            response.body
        );
        assert!(
            response.body.contains("capabilities.prefix"),
            "{label}：错误文案该说清是哪个字段：{}",
            response.body
        );
        let status = http_client::request(addr, "GET", &format!("/sessions/{id}"), None);
        assert_eq!(
            status.status, 404,
            "{label}：被拒的声明不该把会话建出来：{}",
            status.body
        );
    }
}

async fn start(upstream: &FakeServer) -> SocketAddr {
    let template = support::http_server::session_template(upstream.endpoint());
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |c| c,
    )
    .await;
    server.addr
}

fn create(addr: SocketAddr, body: Value) {
    let response = create_raw(addr, body);
    assert_eq!(response.status, 201, "{}", response.body);
}

fn create_raw(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
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

/// 发一句话，等假上游收到那一次 provider 调用，把请求体取出来（单跳，照
/// `http_capabilities_skills.rs::one_turn` 同款手法）。
async fn one_turn(upstream: &FakeServer, addr: SocketAddr, id: &str) -> Value {
    let before = upstream.request_count();
    input(addr, id);
    let deadline = Instant::now() + Duration::from_secs(5);
    while upstream.request_count() == before {
        assert!(
            Instant::now() < deadline,
            "{id}：等假上游收到 provider 调用超时"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let raw = upstream.bodies().swap_remove(before);
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{raw}"))
}

/// 请求体里那条 `role: "system"` 消息的正文。
fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .expect("请求体里该有 messages")
        .iter()
        .find(|m| m["role"] == json!("system"))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}
