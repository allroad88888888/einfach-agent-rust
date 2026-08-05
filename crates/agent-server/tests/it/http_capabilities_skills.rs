//! 064 的核心验收，**端到端、同一个 server 进程**：宿主在 `POST /sessions` 声明的
//! skill 在这个会话里真的活了过来——常驻索引进 system 段、`srv:skill/activate` 进
//! 工具表、正文与自带工具**等激活那一轮**才进。
//!
//! 断言的落点是**假上游收到的请求体**：料对不对的唯一判据是模型看到了什么，不是
//! 内部某个 `Vec` 长什么样（跟 062 的作用域测试同一种证明手法）。
//!
//! 顺带钉住 HOST-CAPABILITIES §八 那个空洞的修复：064 之前，`ToolTableSpec` 五档
//! **全都不接 `.with_skills(..)`**，经 HTTP 起的会话里 `srv:skill/activate` 根本不在
//! 表里——M5 做的整套 skill 机制在 server 形态下等于不存在。

mod support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::Notice;
use agent_server::{Frame, SessionEvent};
use serde_json::{Value, json};

use support::http_client;
use support::server::{FakeServer, Script};

const CRM_BODY: &str = "CRMFLOW_BODY_MARKER_ZX91";
const MAIL_BODY: &str = "MAILFLOW_BODY_MARKER_ZX91";

/// 两个 skill，**故意不按字典序给**（`mail-flow` 在前）：索引必须是排过序的。
fn declaration() -> Value {
    json!({
        "skills": [
            { "id": "mail-flow",
              "description": "发信的标准流程",
              "body": format!("先草拟再发送。{MAIL_BODY}"),
              "tools": [ { "name": "desk:mail/send", "description": "发一封邮件" } ] },
            { "id": "crm-flow",
              "description": "处理客户工单的标准流程",
              "body": format!("先查档案再关单。{CRM_BODY}"),
              "tools": [ { "name": "web:crm/close", "description": "关掉一个工单" } ] }
        ]
    })
}

/// 验收第 1 条 + **作用域**那一条：声明两个 skill → system 段出现**索引两行**、
/// `body` 不在、自带工具不在工具表；另起一个不带声明的会话 → `srv:skill/activate`
/// **不在表里**。
#[tokio::test(flavor = "multi_thread")]
async fn two_declared_skills_become_two_index_lines_and_nothing_more() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start(&upstream).await;

    create(
        addr,
        json!({ "id": "declared", "capabilities": declaration() }),
    );
    create(addr, json!({ "id": "plain" }));

    let declared = one_turn(&upstream, addr, "declared").await;
    let plain = one_turn(&upstream, addr, "plain").await;

    // ── 索引两行，按 id 排序（`crm-flow` 在 `mail-flow` 前面，跟声明数组的顺序相反）。
    let declared_system = system_text(&declared);
    let index: Vec<&str> = declared_system
        .lines()
        .filter(|l| l.contains(": "))
        .collect();
    assert_eq!(
        index,
        vec![
            "crm-flow: 处理客户工单的标准流程",
            "mail-flow: 发信的标准流程"
        ],
        "该有且只有两行索引，按 id 排序（客户端给的数组顺序进不了 prompt，红线 11）"
    );

    // ── 正文不在（延迟加载的全部意义：声明一百个 skill，prompt 里也只多一百行）。
    for marker in [CRM_BODY, MAIL_BODY] {
        assert!(
            !system_text(&declared).contains(marker),
            "激活之前不该看到任何正文：{}",
            system_text(&declared)
        );
    }

    // ── 两个 skill 工具进表，自带的工具**不进**（它们等激活那一轮）。
    assert_eq!(
        names(&declared),
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "srv:skill/activate",
            "srv:skill/deactivate"
        ],
        "skill 两件追加在部署期那一档之后；自带的工具等激活才进 late_tools"
    );

    // ── 作用域：另起一个**不带声明**的会话，整套 skill 机制不该出现在它眼前。
    assert_eq!(
        names(&plain),
        vec!["srv:fs/read", "srv:fs/list"],
        "不带声明的会话该跟 064 之前逐字节一样"
    );
    assert!(
        !system_text(&plain).contains(": "),
        "不带声明的会话不该有任何索引行：{}",
        system_text(&plain)
    );
}

/// 验收第 2 条：模型 `srv:skill/activate` 其中一个 → **那一轮** `late_system` 出现
/// 它的 `body`、`late_tools` 出现它自带的工具；**另一个仍然只有索引行**。
///
/// 「另一个」那一半是这条的正对照：只断言「激活的那个有了」的话，一个「把所有正文
/// 都注入」的实现同样会绿——而那正是延迟加载要避免的。
#[tokio::test(flavor = "multi_thread")]
async fn activating_one_skill_injects_only_its_own_body_and_tools() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(activate_call("crm-flow")),
        Script::Immediate(support::wire::text_reply("激活完毕。")),
    ]);
    let addr = start(&upstream).await;
    create(
        addr,
        json!({ "id": "declared", "capabilities": declaration() }),
    );

    let before = upstream.request_count();
    input(addr, "declared");
    wait_for_turn(&upstream, addr, "declared", before + 2).await;

    let after_activation = body_at(&upstream, before + 1);

    // ── 激活的那个：正文进这一轮，自带的工具进这一轮。
    assert!(
        system_text(&after_activation).contains(CRM_BODY),
        "激活之后的下一跳该带上它的正文：{}",
        system_text(&after_activation)
    );
    assert!(
        names(&after_activation).contains(&"web:crm/close".to_string()),
        "自带的工具该进 late_tools：{:?}",
        names(&after_activation)
    );

    // ── 没激活的那个：**仍然只有索引行**（延迟加载的全部意义）。
    assert!(
        !system_text(&after_activation).contains(MAIL_BODY),
        "没激活的那个不该有正文：{}",
        system_text(&after_activation)
    );
    assert!(
        !names(&after_activation).contains(&"desk:mail/send".to_string()),
        "没激活的那个不该有工具：{:?}",
        names(&after_activation)
    );
    assert!(
        system_text(&after_activation).contains("mail-flow: 发信的标准流程"),
        "它的索引行该还在"
    );
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
    let response = http_client::request(addr, "POST", "/sessions", Some(&body.to_string()));
    assert_eq!(response.status, 201, "{}", response.body);
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

/// 发一句话，等假上游收到那一次 provider 调用，把请求体取出来（单跳）。
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
    body_at(upstream, before)
}

/// 两跳那一轮：靠 SSE 等到终态，避免在「第二跳还没发出去」的时候就去读请求体。
async fn wait_for_turn(upstream: &FakeServer, addr: SocketAddr, id: &str, want: usize) {
    let (_, _, mut sse) = http_client::connect_sse(addr, &format!("/sessions/{id}/events"), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if upstream.request_count() >= want {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(event) = sse.next_event(remaining) else {
            continue;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|e| panic!("SSE 帧不是 Frame：{e}: {}", event.data));
        if matches!(frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal())
        {
            break;
        }
    }
    assert!(
        upstream.request_count() >= want,
        "等第 {want} 次 provider 调用超时，实际 {}",
        upstream.request_count()
    );
}

fn body_at(upstream: &FakeServer, index: usize) -> Value {
    let body = upstream.bodies().swap_remove(index);
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{body}"))
}

/// 请求体里那条 `role: "system"` 消息的正文——模型真正看到的那串字符。
/// DeepSeek 把 `late_system` 折进**同一条** system 消息的尾部（038），所以激活前后
/// 看的都是这一条。
fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .expect("请求体里该有 messages")
        .iter()
        .find(|m| m["role"] == json!("system"))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}

/// wire 上的 `function.name` 是转义过的（050），用 provider 自己那把解码器还原。
fn names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            agent_providers::wire_name::from_wire(
                t["function"]["name"].as_str().unwrap_or_default(),
            )
            .to_string()
        })
        .collect()
}

/// 一段 DeepSeek 形状的流式回复：模型调 `srv:skill/activate({"skill": "<id>"})`。
fn activate_call(id: &str) -> String {
    let args = serde_json::to_string(&json!({ "skill": id }).to_string()).expect("json string");
    format!(
        concat!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":null,",
            "\"tool_calls\":[{{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",",
            "\"function\":{{\"name\":\"srv_3Askill_2Factivate\",\"arguments\":{args}}}}}]}}}}]}}\n\n",
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"\"}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        args = args
    )
}
