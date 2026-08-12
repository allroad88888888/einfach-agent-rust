//! 156 验收「重启恢复……恢复后 spawn 子 inherit_prefix 点声明名成功、点未声明名
//! 拒」——姊妹文件 `http_capabilities_prefix_survive_restart.rs` 证的是「重启这
//! 一步本身」（首轮 body 逐字节一致），这份证的是「重启之后，装配期重新合成出
//! 来的那张表，`srv:agent/spawn` 的 `inherit_prefix` 校验认不认」：那道校验读的
//! 是 timed 区的 spec 名（`check_prefix_allowed`），只有恢复路真的重新
//! `with_host_prefix` 了这份声明，spec 名才会在。
//!
//! 请求体路由用 `support::routed::RoutedServer`（按内容路由，手法照抄
//! `spawn_over_http_interleaves_child_agent_frames.rs`）：一条会话上先后跑「点
//! 声明名 spawn 成功」「点未声明名 spawn 被拒」两轮，用各自消息里独一份的标记
//! 串路由，标记串的先后覆盖关系决定了路由表必须按「越晚出现的标记排越前面」的
//! 顺序声明——模块内 [`routes`] 的注释按调用顺序写清楚了每一条的判据。

use crate::support;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use agent_core::AgentLimits;
use agent_server::{Frame, SessionEvent, ToolTableSpec};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::routed::{Route, RoutedServer};

const CHAT_ID: &str = "prefix-inherit-restart-chat";
const PREFIX_A_NAME: &str = "web:crm/briefing";
const PREFIX_A_TEXT: &str = "PREFIX-INHERIT-A-51fd 今天的客户上下文";
const PREFIX_B_NAME: &str = "desk:ops/standup";
const PREFIX_B_TEXT: &str = "PREFIX-INHERIT-B-9a06 今天的运维简报";
const UNDECLARED_NAME: &str = "web:not-declared-xyz";

const ACCEPT_MARKER: &str = "ACCEPT-TURN-3d8e";
const ACCEPT_CHILD_TASK: &str = "ACCEPT-CHILD-TASK-1c47";
const ACCEPT_CHILD_DONE: &str = "ACCEPT-CHILD-DONE-6b90";
const REJECT_MARKER: &str = "REJECT-TURN-f02a";
/// `spawn_tool::check_prefix_allowed` 的错误文案片段——它就是「点未声明名」这
/// 条判定实际落地的地方，把它当路由 needle 比再造一个标记串更直接。
const REJECTION_TEXT: &str = "不是开局工具名";

fn declaration() -> Value {
    json!({
        "prefix": [
            { "name": PREFIX_A_NAME, "text": PREFIX_A_TEXT },
            { "name": PREFIX_B_NAME, "text": PREFIX_B_TEXT }
        ]
    })
}

fn spawn_wire() -> String {
    agent_providers::wire_name::to_wire("srv:agent/spawn")
}

/// `srv:agent/spawn` 的入参编成 SSE `arguments` 字段要的原文：JSON 对象
/// `to_string()` 之后再当一个 JSON 字符串整体转义一次（手法照抄
/// `agent-runtime/tests/it/inherit_prefix_restore_indep.rs::spawn_input`）。
fn spawn_args(value: Value) -> String {
    let raw = value.to_string();
    let escaped = serde_json::to_string(&raw).expect("字符串序列化不该失败");
    escaped[1..escaped.len() - 1].to_string()
}

/// 一次 `srv:agent/spawn` 工具调用的三行 SSE（tool_calls 增量 + finish_reason +
/// `[DONE]`），手法照抄 `spawn_over_http_interleaves_child_agent_frames.rs::
/// root_spawns_two`。
fn spawn_call_lines(call_id: &str, task: &str, inherit_prefix: Vec<&str>) -> Vec<String> {
    let wire = spawn_wire();
    let args = spawn_args(json!({ "task": task, "inherit_prefix": inherit_prefix }));
    vec![
        format!(
            r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"{call_id}","type":"function","function":{{"name":"{wire}","arguments":"{args}"}}}}]}}}}]}}"#
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#.to_string(),
        "data: [DONE]".to_string(),
    ]
}

/// 一段纯文本、结束这一跳的 SSE。
fn text_lines(content: &str) -> Vec<String> {
    vec![
        format!(
            r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}}}"#
        ),
        "data: [DONE]".to_string(),
    ]
}

/// 六次请求，按**因果顺序**列出（`RoutedServer` 按声明顺序首次匹配子串，越晚
/// 出现在对话里的标记——它的请求体天然也包含更早的标记——必须排得更靠前，见
/// 模块文档）：
///
/// C1 根首跳：用户发 "你好"（无标记）→ 兜底路由，纯文本回复（这一轮只是把
///    会话从「刚恢复」推进到「能继续对话」，不参与下面的断言）。
/// C2 根：用户发 `ACCEPT_MARKER` → 回 spawn 工具调用，
///    `inherit_prefix: [PREFIX_A_NAME]`，任务文本里带 `ACCEPT_CHILD_TASK`。
/// C3 子：子 agent 自己的首跳（user 消息就是 C2 给的任务文本，含
///    `ACCEPT_CHILD_TASK`）→ 纯文本回复，带 `ACCEPT_CHILD_DONE`。
/// C4 根：子结果（`ACCEPT_CHILD_DONE`）回到根的 tool_result 里，根收到之后的
///    下一跳 → 纯文本确认，结束这一轮。
/// C5 根：用户发 `REJECT_MARKER` → 回 spawn 工具调用，
///    `inherit_prefix: [UNDECLARED_NAME]`（没声明过的名字）。
/// C6 根：spawn 在宿主侧同步失败（`check_prefix_allowed` 拒绝，从没有子
///    agent 被造出来），失败文本变成这次调用的 tool_result，根的下一跳请求体
///    里就带着 `REJECTION_TEXT` → 纯文本确认，结束这一轮。
fn routes() -> Vec<Route> {
    vec![
        Route::sse(REJECTION_TEXT, text_lines("root ack reject")), // C6
        Route::sse(
            REJECT_MARKER,
            spawn_call_lines("call_reject", "reject task", vec![UNDECLARED_NAME]),
        ), // C5
        Route::sse(ACCEPT_CHILD_DONE, text_lines("root ack accept")), // C4
        Route::sse(ACCEPT_CHILD_TASK, text_lines(ACCEPT_CHILD_DONE)), // C3
        Route::sse(
            ACCEPT_MARKER,
            spawn_call_lines(
                "call_accept",
                &format!("do the accept task {ACCEPT_CHILD_TASK}"),
                vec![PREFIX_A_NAME],
            ),
        ), // C2
        Route::sse("", text_lines("好的")), // C1，兜底
    ]
}

/// 恢复后 spawn 认不认 `inherit_prefix`：点声明名（`PREFIX_A_NAME`）成功，
/// 子 agent 真的带着 A 的文本（且不带 B 的——`inherit_prefix` 是从严白名单，
/// 不是「至少带一个就算数」）；点未声明名（`UNDECLARED_NAME`）被拒，而且全程
/// 没有任何子 agent 被造出来。
#[tokio::test(flavor = "multi_thread")]
async fn spawn_after_restart_accepts_a_declared_name_and_rejects_an_undeclared_one() {
    let sessions_dir = support::temp_dir("prefix-inherit-restart");

    // ── 第一次：只是把声明写进 store、关掉——这份测试不关心第一次进程里的行为，
    //    姊妹文件 `..._survive_restart.rs` 已经证过那一半。
    let bootstrap = support::server::FakeServer::start(vec![script_immediate_ok()]);
    let (first_addr, first_sessions) = start(persistent_template(
        bootstrap.endpoint(),
        sessions_dir.clone(),
        ToolTableSpec::Full {
            spawn_limits: AgentLimits::default(),
        },
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
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    // ── 第二次：**不带任何 `capabilities`**，重开。装配期从 `Slot::HostPrefix`
    //    读回声明、重新 `with_host_prefix` 合成 timed 条目——这是本条要证的机制。
    let routed = RoutedServer::start(routes());
    let (addr, sessions) = start(persistent_template(
        routed.endpoint(),
        sessions_dir,
        ToolTableSpec::Full {
            spawn_limits: AgentLimits::default(),
        },
    ))
    .await;
    let recovered = create(addr, json!({ "id": CHAT_ID }));
    assert_eq!(recovered.status, 200, "{}", recovered.body);

    let (_, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{CHAT_ID}/events"), None);

    input(addr, "你好"); // C1
    wait_for_terminal(&mut sse);

    input(addr, &format!("go: {ACCEPT_MARKER}")); // C2 → C3 → C4
    wait_for_terminal(&mut sse); // 这一轮有子 agent：等的是 **root** 的终态帧，
    //  子 agent 自己的终态帧会先到，不能被当成整轮结束（见 wait_for_terminal）。

    input(addr, &format!("go: {REJECT_MARKER}")); // C5 → C6
    wait_for_terminal(&mut sse);

    // ── 收：子 agent 真的带着 PREFIX_A 的文本，不带 PREFIX_B 的。
    let child_call = routed
        .call(ACCEPT_CHILD_TASK)
        .expect("子 agent 该真的发起了自己的第一跳请求");
    assert!(
        child_call.body.contains(PREFIX_A_TEXT),
        "inherit_prefix 点了 A，子的 system 段该带上它：{}",
        child_call.body
    );
    assert!(
        !child_call.body.contains(PREFIX_B_TEXT),
        "没点 B，子的 system 段不该带上它——inherit_prefix 是从严白名单：{}",
        child_call.body
    );

    // ── 拒：根收到了带 `REJECTION_TEXT`、点名 `UNDECLARED_NAME` 的 tool_result。
    let rejection_call = routed
        .call(REJECTION_TEXT)
        .expect("根该在 spawn 被拒之后自动带着错误文本继续这一轮");
    assert!(
        rejection_call.body.contains(UNDECLARED_NAME),
        "错误文案该点名是哪个未声明的名字：{}",
        rejection_call.body
    );

    // ── 全程没有第二个子 agent：拒绝发生在宿主侧同步校验，从没有走到「子
    //    agent 发起自己的请求」这一步——六条路由各命中一次，不多不少。
    assert_eq!(
        routed.calls().len(),
        6,
        "该恰好六次请求（C1..C6），被拒的 spawn 不该多出一次子请求：{:?}",
        routed
            .calls()
            .iter()
            .map(|c| c.needle)
            .collect::<Vec<_>>()
    );

    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// 一段最简单的纯文本 SSE 脚本——第一次进程只用来落声明，不参与任何断言。
fn script_immediate_ok() -> support::server::Script {
    support::server::Script::Immediate(support::wire::text_reply("好的。"))
}

async fn start(
    template: agent_server::SessionTemplate,
) -> (SocketAddr, agent_server::SessionsHandle) {
    let server = agent_server::AgentServer::new(
        agent_server::ServerConfig::new(template)
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

fn persistent_template(
    endpoint: String,
    sessions_dir: PathBuf,
    tools: ToolTableSpec,
) -> agent_server::SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.default_sessions_dir = Some(sessions_dir);
    template.tools = tools;
    template
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

fn input(addr: SocketAddr, text: &str) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(&json!({ "text": text }).to_string()),
    );
    assert_eq!(response.status, 202, "{}", response.body);
}

/// 等**root**（`frame.agent.as_str() == "root"`）落终态——这条会话里有 spawn，
/// 子 agent 自己也会广播一条 `TurnStatusChanged` 终态帧（标它自己的归属），
/// 且必然先于 root 的到达（root 要等子收工才能继续）。不按归属过滤的话，第一次
/// 见到的终态帧会是子的，调用方会把「子刚收工、root 还在等 provider 回它的
/// 下一跳」误判成「整轮结束」——这份测试恰恰全程都在等 root 的下一跳
/// （`一_turn` 的每一步都要真的发生），提前返回会让后续的 `input()` 在 root
/// 仍忙着的时候插队,虽然消息本身不会丢（actor 的命令队列是 FIFO），但会让
/// 这里的「等」名不副实。
fn wait_for_terminal(sse: &mut http_client::SseReader) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(event) = sse.next_event(remaining) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|e| panic!("SSE 帧不是 Frame：{e}: {}", event.data));
        if frame.agent.as_str() != "root" {
            continue;
        }
        if matches!(
            frame.event,
            SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status })
                if status.is_terminal()
        ) {
            return;
        }
    }
    panic!("等待回合终态超时");
}
