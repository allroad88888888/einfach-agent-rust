//! 073 的核心验收，**端到端、跨进程形态**：宿主建会话时声明的工具**进 store**，
//! 会话关掉再按同一个 chatid 打开时**不带任何 `capabilities` 也原样回来**。
//!
//! > 历史对话记录，不用对工具再注入一次。历史对话就该跟历史一致，原模原样 100% 复刻。
//!
//! 断言的落点跟 062 一样是**假上游收到的请求体**——工具表最终的用处就是变成 prompt
//! 字节，「恢复出来还有没有这个工具」只有在这里才算真的证到。三条各自可判定：
//!
//! 1. 恢复后不带声明，表里**仍然有**注入的工具；
//! 2. 恢复后第一轮的 `tools` 段与关闭前那一轮**逐字节相同**（红线 11 的真意——
//!    不是「有这个工具」，是「一个字节都没变」，前缀缓存才接得上）；
//! 3. 恢复时又带声明 → **直接拒绝**，且错误码能跟「名字写错了」分开。

use crate::support;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use agent_core::Notice;
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::{Value, json};

use crate::support::http_client::{self, SseReader};
use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

const CHAT_ID: &str = "caps-restart-chat";

/// 两个工具，**故意不按字典序给**（062 那份的同款）：进表要按名字排序。
fn declaration() -> Value {
    json!({
        "tools": [
            { "name": "web:crm/lookup",
              "description": "按客户 ID 查 CRM 档案",
              "schema": { "type": "object", "properties": { "id": { "type": "string" } } },
              "reversibility": "pure" },
            { "name": "desk:clipboard/write", "description": "写系统剪贴板" }
        ]
    })
}

/// **本 issue 的全部意义**：建会话 + 注入 → 对话一轮 → 关掉 → 同 chatid 重开、
/// **不带任何 `capabilities`** → 工具表里仍然有那两个工具，而且 `tools` 段与关闭前
/// **逐字节相同**。
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_session_brings_its_declared_tools_back_without_being_told_again() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-restart");

    // ── 第一次：带声明建会话，跑一轮。
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
    assert_eq!(
        support::extract_json_string_field(&created.body, "outcome"),
        "created"
    );
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

    // ── 1. 注入的工具还在（`declares()` 在 prompt 上的投影就是这一条）。
    assert_eq!(
        names(&after),
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "desk:clipboard/write",
            "web:crm/lookup"
        ],
        "恢复出来的会话该带回它自己当初的那份工具表，宿主不必也不该再声明一遍"
    );

    // ── 2. 逐字节相同（红线 11）。
    assert_eq!(
        serde_json::to_string(&after).unwrap(),
        serde_json::to_string(&before).unwrap(),
        "恢复后第一轮的工具表字节必须与关闭前最后一轮相同，否则恢复出来的会话第一轮就前缀全断"
    );

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// 恢复时客户端又带 `capabilities` → **直接拒绝**（用户拍板），而且错误码要能让
/// 它把「我名字写错了」和「这会话已有历史」分开——两者都是 400，光看状态码分不出
/// 来，而正确的应对完全相反（改名字重发 vs. 去掉声明重发）。
#[tokio::test(flavor = "multi_thread")]
async fn declaring_again_on_a_session_with_history_is_refused_with_its_own_error_code() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-refuse");

    let (first_addr, first_sessions) = start(persistent_template(
        upstream.endpoint(),
        sessions_dir.clone(),
    ))
    .await;
    assert_eq!(
        create(
            first_addr,
            json!({ "id": CHAT_ID, "capabilities": declaration() })
        )
        .status,
        201
    );
    one_turn(&upstream, first_addr).await;
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    let (addr, sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;

    // ── 又带一次声明（哪怕跟当初一模一样）：拒。
    let refused = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": declaration() }),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused.body.contains("\"session_has_history\""),
        "要有自己的错误码，不能只是通用 bad_request：{}",
        refused.body
    );
    assert!(
        refused.body.contains("GET /sessions/"),
        "错误文案该告诉调用方怎么判断该不该带声明：{}",
        refused.body
    );
    assert!(
        sessions.ids().is_empty(),
        "被拒的请求不该把会话 open 起来：{:?}",
        sessions.ids()
    );

    // ── 同一个会话、名字写错的声明：**另一个**错误码（这才叫「能区分」）。
    let bad_name = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": { "tools": [ { "name": "srv:crm/lookup" } ] } }),
    );
    assert_eq!(bad_name.status, 400, "{}", bad_name.body);
    assert!(
        bad_name.body.contains("\"session_has_history\""),
        "有历史这条排在名字校验前面——这次的声明再正确也不会被采纳，先说这个更有用：{}",
        bad_name.body
    );

    // ── 不带声明：照常恢复，工具还在（拒绝没有把会话弄坏）。
    let ok = create(addr, json!({ "id": CHAT_ID }));
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert!(names(&one_turn(&upstream, addr).await).contains(&"web:crm/lookup".to_string()));
    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// 「先查再建」契约的地基：`GET /sessions/{id}` 必须把**有历史但此刻没活着**
/// （= 恰恰是恢复那种情况）跟**从没见过**分开。分不开的话，客户端照 404 判定
/// 「新会话」于是带上声明，然后被上面那条拒绝顶回来——契约当场作废。
#[tokio::test(flavor = "multi_thread")]
async fn the_status_endpoint_tells_a_chatid_with_history_from_one_without() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-status");
    let (addr, sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;

    // 建之前：404 = 没有任何历史 = **该带声明**。
    let unknown = http_client::request(addr, "GET", &format!("/sessions/{CHAT_ID}"), None);
    assert_eq!(unknown.status, 404, "{}", unknown.body);

    assert_eq!(
        create(
            addr,
            json!({ "id": CHAT_ID, "capabilities": declaration() })
        )
        .status,
        201
    );
    one_turn(&upstream, addr).await;
    let alive = http_client::request(addr, "GET", &format!("/sessions/{CHAT_ID}"), None);
    assert_eq!(alive.status, 200, "{}", alive.body);
    assert!(alive.body.contains("\"alive\""), "{}", alive.body);

    // 关掉之后：registry 里没有了，但磁盘上有——**不是 404**，是 dormant。
    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));
    let dormant = http_client::request(addr, "GET", &format!("/sessions/{CHAT_ID}"), None);
    assert_eq!(
        dormant.status, 200,
        "关掉的会话磁盘上还有历史，答 404 就等于骗客户端「这是新会话，带上声明吧」：{}",
        dormant.body
    );
    assert!(dormant.body.contains("\"dormant\""), "{}", dormant.body);

    // 没有历史的另一个 chatid 仍然是 404（dormant 不是把 404 一并吃掉了）。
    let other = http_client::request(addr, "GET", "/sessions/never-existed", None);
    assert_eq!(other.status, 404, "{}", other.body);
}

/// **undo 一致性**：声明发生在会话建立那一步，`undo` 到它之前 → 工具表回到没有
/// 注入的状态。这条按设计是「白拿」（走的是同一条 journaled 路），所以这里要有
/// 断言证明它真的白拿了，而不是嘴上说说。
///
/// 三次观察，中间那次是**正对照**——只断言「undo 之后没有了」是自欺欺人：一个
/// 「从来就没恢复过任何工具」的实现同样会绿。
///
/// | 观察 | 这一刻的历史 | 该看到 |
/// |---|---|---|
/// | A | 声明 + 一轮对话 | 有注入 |
/// | B | undo 掉那一轮对话之后重开 | **仍有**注入（声明自成一轮，没被误伤） |
/// | C | 再 undo 两轮（新那轮 + 声明那轮）之后重开 | **没有**注入 |
///
/// 「重开」这一步不能省：工具表在 actor 起来时装配一次，之后这个运行实例内不再变
/// （HOST-CAPABILITIES §三 不做运行时增删）。undo 改的是**会话状态**，它对工具表
/// 的作用要等下一次装配才看得见——这正是「恢复 = 忠实重放」的另一面。
#[tokio::test(flavor = "multi_thread")]
async fn undoing_past_the_declaration_takes_the_injected_tools_out_of_the_table() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("caps-undo");

    // ── A：声明 + 一轮对话，然后 undo 掉那一轮对话。
    let (a_addr, a_sessions) = start(persistent_template(
        upstream.endpoint(),
        sessions_dir.clone(),
    ))
    .await;
    assert_eq!(
        create(
            a_addr,
            json!({ "id": CHAT_ID, "capabilities": declaration() })
        )
        .status,
        201
    );
    let a = one_turn(&upstream, a_addr).await;
    assert!(names(&a).contains(&"web:crm/lookup".to_string()), "{a:#?}");
    undo(a_addr);
    assert!(a_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    // ── B（正对照）：撤掉一轮**对话**没有动到声明——重开之后工具还在。
    let (b_addr, b_sessions) = start(persistent_template(
        upstream.endpoint(),
        sessions_dir.clone(),
    ))
    .await;
    assert_eq!(create(b_addr, json!({ "id": CHAT_ID })).status, 200);
    let b = one_turn(&upstream, b_addr).await;
    assert!(
        names(&b).contains(&"web:crm/lookup".to_string()),
        "undo 一轮对话不该顺手把宿主的声明也撤掉——声明是它自己那一轮：{b:#?}"
    );
    // 再撤两轮：刚才这一轮对话，以及**声明那一轮**。
    undo(b_addr);
    undo(b_addr);
    assert!(b_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    // ── C：undo 到注入发生之前 → 工具表回到没有注入的状态。
    let (c_addr, c_sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;
    assert_eq!(create(c_addr, json!({ "id": CHAT_ID })).status, 200);
    let c = one_turn(&upstream, c_addr).await;
    assert_eq!(
        names(&c),
        vec!["srv:fs/read", "srv:fs/list"],
        "undo 越过声明那一步之后，这个会话的工具表该回到没有注入的样子：{c:#?}"
    );
    assert!(c_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// `POST /sessions/:id/undo`（turn 档，不 force）。命令进的是 actor 的队列，
/// 后面的 `close_all` 排在它之后，所以关掉的时候这一步必然已经落定——不必再等
/// 一条 SSE 事件。
fn undo(addr: SocketAddr) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/undo"),
        Some(r#"{"granularity":"turn"}"#),
    );
    assert_eq!(response.status, 202, "{}", response.body);
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

/// 发一句话、等这一轮走到终态，把这次 provider 调用请求体里的 `tools` 数组取出来。
async fn one_turn(upstream: &FakeServer, addr: SocketAddr) -> Vec<Value> {
    let before = upstream.request_count();
    let (_, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{CHAT_ID}/events"), None);
    let input = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(r#"{"text":"你好"}"#),
    );
    assert_eq!(input.status, 202, "{}", input.body);
    wait_for_terminal(&mut sse);

    let body = upstream.bodies().swap_remove(before);
    let body: Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{body}"));
    body["tools"].as_array().cloned().unwrap_or_default()
}

/// wire 上的 `function.name` 是转义过的（050），用 provider 自己那把解码器还原。
fn names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .map(|t| {
            agent_providers::wire_name::from_wire(
                t["function"]["name"].as_str().unwrap_or_default(),
            )
            .to_string()
        })
        .collect()
}

fn wait_for_terminal(sse: &mut SseReader) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(event) = sse.next_event(remaining) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|e| panic!("SSE 帧不是 Frame：{e}: {}", event.data));
        if matches!(frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal())
        {
            return;
        }
    }
    panic!("等待回合终态超时");
}
