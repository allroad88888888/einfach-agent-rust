//! 064 §验收「恢复」与「`/undo` 撤掉激活」，**端到端、跨进程形态**（形状照 073 的
//! `http_capabilities_survive_restart.rs`）。
//!
//! 用户拍板的那条原则对 skill 一视同仁：
//!
//! > 历史对话记录，不用对工具再注入一次。**历史对话就该跟历史一致，原模原样 100% 复刻。**
//!
//! skill 声明**也进 store**（`Slot::HostSkills`，journaled），理由比工具那一路更硬：
//! `Slot::SkillsActive` 早就在 store 里了——声明不落盘，恢复出来就是一份**指向空
//! registry 的激活集**（状态说某个 skill 激活着、展开注入却什么都取不到，而模型的
//! 历史里明明写着它读过那段正文）。而且 073 之后有历史的会话再带 `capabilities`
//! 一律 400，**不存下来就是永久没了**，连「重连时重报一遍」这条退路都不存在。

use crate::support;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_server::{AgentServer, ServerConfig, SessionTemplate, SessionsHandle};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CHAT_ID: &str = "caps-skill-restart";
const CRM_BODY: &str = "CRMFLOW_BODY_MARKER_ZX91";

fn declaration() -> Value {
    json!({
        "skills": [
            { "id": "mail-flow", "description": "发信的标准流程", "body": "先草拟再发送。" },
            { "id": "crm-flow",
              "description": "处理客户工单的标准流程",
              "body": format!("先查档案再关单。{CRM_BODY}"),
              "tools": [ { "name": "web:crm/close", "description": "关掉一个工单" } ] }
        ]
    })
}

/// **本条的全部意义**：建会话 + 声明两个 skill → 对话一轮 → 关掉 → 同 chatid 重开、
/// **不带任何 `capabilities`** → 常驻索引与 `srv:skill/activate` 都还在，而且 system
/// 段那两行**逐字节相同**（红线 11 的真意——不是「有这个 skill」，是「一个字节都
/// 没变」，前缀缓存才接得上）。
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_session_brings_its_declared_skills_back_without_being_told_again() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-skill-restart");

    let (first_addr, first_sessions) = start(persistent_template(
        upstream.endpoint(),
        sessions_dir.clone(),
    ))
    .await;
    let created = create(
        first_addr,
        json!({ "id": CHAT_ID, "capabilities": declaration() }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let before = one_turn(&upstream, first_addr).await;
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    // ── 第二次：**请求体里一个 `capabilities` 字都没有**。
    let (second_addr, second_sessions) =
        start(persistent_template(upstream.endpoint(), sessions_dir)).await;
    let recovered = create(second_addr, json!({ "id": CHAT_ID }));
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(
        support::extract_json_string_field(&recovered.body, "outcome"),
        "recovered"
    );
    let after = one_turn(&upstream, second_addr).await;

    assert!(
        names(&after).contains(&"srv:skill/activate".to_string()),
        "恢复出来的会话该带回它自己当初那套 skill——registry 空了的话这个工具连表都进不了：{:?}",
        names(&after)
    );
    assert_eq!(
        system_text(&after),
        system_text(&before),
        "恢复后第一轮的 system 段必须与关闭前那一轮逐字节相同，否则恢复出来的会话第一轮就前缀全断（红线 11）"
    );
    assert!(
        system_text(&after).contains("crm-flow: 处理客户工单的标准流程"),
        "{}",
        system_text(&after)
    );

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// 验收「`/undo` 撤掉激活 → 下一轮 `body` 消失」（journaled，既有机制白拿——但要有
/// 断言证明它真的白拿了）。
///
/// 三次观察，**中间那次是正对照**：只断言「undo 之后正文没了」是自欺欺人，一个
/// 「从来就没注入过正文」的实现同样会绿。顺带钉住 073 那条「声明自成一轮」在 skill
/// 这一路也成立：撤掉对话那一轮之后，**索引行还在**（声明没被误伤）。
#[tokio::test(flavor = "multi_thread")]
async fn undoing_the_activation_takes_the_body_out_of_the_next_round() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(activate_call("crm-flow")),
        Script::Immediate(support::wire::text_reply("激活完毕。")),
    ]);
    let sessions_dir = support::temp_dir("caps-skill-undo");
    let (addr, sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;
    assert_eq!(
        create(
            addr,
            json!({ "id": CHAT_ID, "capabilities": declaration() })
        )
        .status,
        201
    );

    // ── A：模型激活 crm-flow，同一轮的下一跳带上它的正文。
    let before = upstream.request_count();
    input(addr);
    wait_for(&upstream, before + 2).await;
    let activated = body_at(&upstream, before + 1);
    assert!(
        system_text(&activated).contains(CRM_BODY),
        "激活之后的下一跳该带上正文：{}",
        system_text(&activated)
    );

    // ── B：再说一句话，正文**仍然在**（激活是会话状态，不是那一跳的临时料）。
    let still = one_turn(&upstream, addr).await;
    assert!(
        system_text(&still).contains(CRM_BODY),
        "激活是 journaled 的会话状态，下一轮该还在：{}",
        system_text(&still)
    );

    // ── C：撤掉两轮（刚才这一轮对话 + 激活那一轮）→ 正文消失，索引行还在。
    undo(addr);
    undo(addr);
    let after_undo = one_turn(&upstream, addr).await;
    assert!(
        !system_text(&after_undo).contains(CRM_BODY),
        "undo 越过激活之后正文该消失：{}",
        system_text(&after_undo)
    );
    assert!(
        system_text(&after_undo).contains("crm-flow: 处理客户工单的标准流程"),
        "撤的是激活，不是宿主的声明——索引行必须还在（声明自成一轮）：{}",
        system_text(&after_undo)
    );

    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

async fn start(template: SessionTemplate) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(ServerConfig::new(template));
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

fn persistent_template(endpoint: String, sessions_dir: PathBuf) -> SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.default_sessions_dir = Some(sessions_dir);
    template
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

fn input(addr: SocketAddr) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(r#"{"text":"你好"}"#),
    );
    assert_eq!(response.status, 202, "{}", response.body);
}

fn undo(addr: SocketAddr) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/undo"),
        Some(r#"{"granularity":"turn"}"#),
    );
    assert_eq!(response.status, 202, "{}", response.body);
}

/// 发一句话，等假上游收到那一次 provider 调用，把请求体取出来（单跳）。
async fn one_turn(upstream: &FakeServer, addr: SocketAddr) -> Value {
    let before = upstream.request_count();
    input(addr);
    wait_for(upstream, before + 1).await;
    body_at(upstream, before)
}

/// 等到假上游收满 `want` 次调用。**判据是请求数而不是一条 SSE 终态帧**：
/// `GET /events` 会补发环形缓冲里的历史帧，上一轮的终态会被当场读到，于是
/// 「等这一轮结束」变成「立刻返回」——那条坑踩过一次，症状是读请求体时越界。
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
