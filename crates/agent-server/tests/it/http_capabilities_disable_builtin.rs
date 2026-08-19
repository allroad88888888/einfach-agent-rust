//! 076 的核心验收，**端到端、同一个 server 进程**：`POST /sessions` 里点名关掉的
//! 内置工具，在这个会话里**模型压根看不见**；另起一个不关的会话照旧有；名字写错了
//! 当场 400 且点名。
//!
//! 断言的落点是**假上游收到的请求体**：料对不对的唯一判据是模型看到了什么，不是
//! 内部某个 `Vec` 长什么样（跟 062/064 的作用域测试同一种证明手法）。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::AgentLimits;
use agent_server::{SessionTemplate, ToolTableSpec};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

/// 关掉表尾那两件（编排的一半 + 那件最该关的 shell）。
fn switch() -> Value {
    json!({ "disable_builtin": ["srv:agent/spawn", "srv:shell/exec"] })
}

/// **验收第 1 条**：关掉 `srv:agent/spawn` → 那个会话的工具表里没有它、
/// **描述也不在进 prompt 的那份字节里**；同一个 server 上另起一个不关的会话 →
/// 照旧有。
///
/// 三段断言各拦一种坏实现：
/// - 名字不在 → 最基本的那条；
/// - **描述不在**（`spawn` 描述里那句「交给一个新的子 agent」）→ 一个「只摘名字」的
///   实现会让描述留在 prompt 里，那笔钱照付、模型还被那段文字影响；
/// - 另一个会话照旧有 → 开关是 **per-session** 的，没粘到全进程那一份
///   `SessionTemplate` 上。这一条尤其要紧：粘上去就是「A 客户端关掉的工具 B 客户端
///   也没了」，而 B 从没提过这个要求，它少掉的那些模型压根不知道存在过。
#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_builtin_is_invisible_here_and_untouched_next_door() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start(&upstream).await;

    create(
        addr,
        json!({ "id": "reduced", "capabilities": switch() }),
        201,
    );
    create(addr, json!({ "id": "plain" }), 201);

    let reduced = one_turn(&upstream, addr, "reduced").await;
    let plain = one_turn(&upstream, addr, "plain").await;

    assert_eq!(
        names(&reduced),
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "srv:agent/status",
            "srv:agent/collect",
            "srv:agent/send",
            "srv:agent/self",
            "srv:agent/notes",
            "srv:agent/notes/set",
            "srv:agent/await"
        ],
        "关掉的那两件不该在表里，没点名的一件不许少（顺序也不许变——红线 11）"
    );
    let bytes = tools_bytes(&reduced);
    for hint in ["交给一个新的子 agent", "sh -c"] {
        assert!(
            !bytes.contains(hint),
            "关掉的工具描述还在进 prompt 的字节里（「{hint}」）——名字摘了描述留着，那笔钱照付"
        );
    }

    assert_eq!(
        names(&plain),
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "srv:shell/exec",
            "srv:agent/spawn",
            "srv:agent/status",
            "srv:agent/collect",
            "srv:agent/send",
            "srv:agent/self",
            "srv:agent/notes",
            "srv:agent/notes/set",
            "srv:agent/await"
        ],
        "隔壁那个不带开关的会话该是完整的一档——开关是 per-session 的，不粘在全局 template 上"
    );
}

/// **验收第 2 条（只能减不能加）**：请求里写一个这个部署没装配的名字 → **400**，
/// 报文点名。
///
/// 三种写法各测一遍，它们是三种不同的错法：
/// - 拼错（`spawnn`）——最常见的那种；
/// - 部署方本来就没开的（`Builtin` 档没有 `srv:shell/exec`，这里用一个别的档才有的
///   名字）——「天花板是**这个部署**的表」；
/// - 一个宿主自己注入的 `web:` 工具——注入的不在天花板里（不想给就别报）。
///
/// **为什么必须报错**：静默忽略的话客户端以为关掉了、其实没关，模型照样调得到
/// `srv:shell/exec`，**没有任何报错**——这一刻客户端还在线、能改，所以该在这里失败。
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_name_is_rejected_by_name() {
    let upstream = FakeServer::start(vec![]);
    let addr = start(&upstream).await;

    for (label, caps, offender) in [
        (
            "拼错",
            json!({ "disable_builtin": ["srv:agent/spawnn"] }),
            "srv:agent/spawnn",
        ),
        (
            "这个部署没装配",
            json!({ "disable_builtin": ["read_file"] }),
            "read_file",
        ),
        (
            "宿主自己注入的（不在天花板里）",
            json!({ "tools": [ { "name": "web:crm/lookup" } ], "disable_builtin": ["web:crm/lookup"] }),
            "web:crm/lookup",
        ),
    ] {
        let response = post(
            addr,
            json!({ "id": format!("bad-{}", offender.len()), "capabilities": caps }),
        );
        assert_eq!(
            response.status, 400,
            "{label}：该 400，实际 {} {}",
            response.status, response.body
        );
        assert!(
            response.body.contains(offender),
            "{label}：报文必须点名是哪一个：{}",
            response.body
        );
        assert_eq!(
            support::extract_json_string_field(&response.body, "code"),
            "bad_request",
            "{label}：名字写错了是 `bad_request`（改名字重发），不是 `session_has_history`（去掉声明重发）——两者应对相反"
        );
    }

    // 拒绝发生在 `open` **之前**：坏请求不该留下一个会话。
    let probe = http_client::request(addr, "GET", "/sessions/bad-15", None);
    assert_eq!(
        probe.status, 404,
        "被拒的请求不该留下任何会话：{}",
        probe.body
    );
}

/// 关掉**全部**编排三件之后，`srv:agent/spawn` 连 `declares()` 都不为真——于是模型
/// 硬猜这个名字也长不出子 agent 树，走的是跟别的不存在的工具同一条 `unknown_tool`。
///
/// 这条是「不启用 ≠ 看得见但不给调」那句话在端到端的落点：判据不是「模型没调」，
/// 是**服务端对这次调用的回答**。
#[tokio::test(flavor = "multi_thread")]
async fn guessing_a_disabled_tool_name_falls_through_to_unknown_tool() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(spawn_call()),
        Script::Immediate(support::wire::text_reply("那我自己来。")),
    ]);
    let addr = start(&upstream).await;
    create(
        addr,
        json!({ "id": "reduced", "capabilities": switch() }),
        201,
    );

    let before = upstream.request_count();
    input(addr, "reduced");
    wait_for(&upstream, before + 2).await;

    // 第二跳的请求体里带着那次调用的结果：模型猜的那个名字被当成不存在的工具。
    let second = body_at(&upstream, before + 1);
    let text = serde_json::to_string(&second["messages"]).expect("messages 该可序列化");
    assert!(
        text.contains("unknown_tool"),
        "关掉的工具被模型硬猜到时该落 `unknown_tool`（跟任何不存在的工具一视同仁），实际：{text}"
    );
}

async fn start(upstream: &FakeServer) -> SocketAddr {
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        full_template(upstream.endpoint()),
        |c| c,
    )
    .await;
    server.addr
}

/// 部署期开满档——本 issue 要关的正是这一档里的东西（`Builtin` 只有两个只读工具，
/// 关不出什么名堂）。
fn full_template(endpoint: String) -> SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.tools = ToolTableSpec::Full {
        spawn_limits: AgentLimits::default(),
    };
    template
}

fn post(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

fn create(addr: SocketAddr, body: Value, want: u16) {
    let response = post(addr, body);
    assert_eq!(response.status, want, "{}", response.body);
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

/// 判据是**假上游收到的请求数**而不是一条 SSE 终态帧（064 踩过的坑：`GET /events`
/// 会补发环形缓冲里的历史帧，上一轮的终态会被当场读到）。
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

/// 整个 `tools` 段的文本——**描述也在里面**，「名字摘了描述留着」那种实现只有比这
/// 一段才抓得住。
fn tools_bytes(body: &Value) -> String {
    serde_json::to_string(&body["tools"]).expect("tools 段该可序列化")
}

/// 一段 DeepSeek 形状的流式回复：模型调一个**已经被关掉**的 `srv:agent/spawn`。
fn spawn_call() -> String {
    let args =
        serde_json::to_string(&json!({ "task": "随便干点什么" }).to_string()).expect("json string");
    format!(
        concat!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":null,",
            "\"tool_calls\":[{{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",",
            "\"function\":{{\"name\":\"srv_3Aagent_2Fspawn\",\"arguments\":{args}}}}}]}}}}]}}\n\n",
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"\"}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        args = args
    )
}
