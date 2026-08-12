//! 064 的核心验收，**端到端、同一个 server 进程**：宿主在 `POST /sessions` 声明的
//! skill 在这个会话里真的活了过来——常驻索引进 system 段、`srv:skill/read` 进
//! 工具表、正文按 id 现读才进对话。
//!
//! 断言的落点是**假上游收到的请求体**：料对不对的唯一判据是模型看到了什么，不是
//! 内部某个 `Vec` 长什么样（跟 062 的作用域测试同一种证明手法）。
//!
//! 顺带钉住 HOST-CAPABILITIES §八 那个空洞的修复：064 之前，`ToolTableSpec` 五档
//! **全都不接 `.with_skills(..)`**，经 HTTP 起的会话里 skill 工具根本不在表里——
//! M5 做的整套 skill 机制在 server 形态下等于不存在。
//!
//! # 139/141 更新：装配从 activate/deactivate 换成 read/index，机制本身也删了
//!
//! `with_skills` 不再往表里塞 `srv:skill/activate`/`deactivate`，模型没有工具调用
//! 能把正文/自带工具注入料单的正文段/中途工具段了——141 把那条通路（连同
//! `Ingredients` 那个正文段字段本身）整个删掉。新机制是 `srv:skill/read`：正文
//! 回到的是**这一跳的 tool_result**，不是常驻 system 段；read 不碰
//! `Slot::SkillsActive`，自带工具因此永远不进表。
//!
//! # 140 更新：skill 不再能声明自带工具
//!
//! 139 让「skill 自带工具永远不进表」成了事实，140 把它钉成**声明时就拒绝**
//! （决策 27：v1 不支持，`capabilities.skills[..].tools` 非空整份 400，独立覆盖见
//! `http_capabilities_declaration.rs` + `host_skill_reject_indep.rs`）。下面
//! `declaration()` 因此不再给 skill 挂 `tools`——这份声明本身现在也是「合法声明」
//! 的样本之一，不是刻意留了个没用的字段。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::Notice;
use agent_server::{Frame, SessionEvent};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CRM_BODY: &str = "CRMFLOW_BODY_MARKER_ZX91";
const MAIL_BODY: &str = "MAILFLOW_BODY_MARKER_ZX91";

/// 两个 skill，**故意不按字典序给**（`mail-flow` 在前）：索引必须是排过序的。
fn declaration() -> Value {
    json!({
        "skills": [
            { "id": "mail-flow",
              "description": "发信的标准流程",
              "body": format!("先草拟再发送。{MAIL_BODY}") },
            { "id": "crm-flow",
              "description": "处理客户工单的标准流程",
              "body": format!("先查档案再关单。{CRM_BODY}") }
        ]
    })
}

/// 验收第 1 条 + **作用域**那一条：声明两个 skill → system 段出现**索引两行**、
/// `body` 不在、自带工具不在工具表；另起一个不带声明的会话 → `srv:skill/read`
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
    // 139：索引格式换成 `index_text()` 的「id — 描述」（em dash），不再是旧的
    // 「id: 描述」——用 " — " 这个分隔符筛行，天然滤掉引导语那一行（它不含这个
    // 分隔符）。
    let declared_system = system_text(&declared);
    let index: Vec<&str> = declared_system
        .lines()
        .filter(|l| l.contains(" — "))
        .collect();
    assert_eq!(
        index,
        vec![
            "crm-flow — 处理客户工单的标准流程",
            "mail-flow — 发信的标准流程"
        ],
        "该有且只有两行索引，按 id 排序（客户端给的数组顺序进不了 prompt，红线 11）"
    );

    // ── 正文不在（延迟加载的全部意义：声明一百个 skill，prompt 里也只多一百行）。
    for marker in [CRM_BODY, MAIL_BODY] {
        assert!(
            !system_text(&declared).contains(marker),
            "读之前不该看到任何正文：{}",
            system_text(&declared)
        );
    }

    // ── skill 只带来一个工具（read）——140 起 skill 声明本身就不能带 `tools`
    //    了（决策 27，非空整份 400，见 `http_capabilities_declaration.rs`）。
    assert_eq!(
        names(&declared),
        vec!["srv:fs/read", "srv:fs/list", "srv:skill/read"],
        "skill 只追加 read 这一件，在部署期那一档之后"
    );

    // ── 作用域：另起一个**不带声明**的会话，整套 skill 机制不该出现在它眼前。
    assert_eq!(
        names(&plain),
        vec!["srv:fs/read", "srv:fs/list"],
        "不带声明的会话该跟 064 之前逐字节一样"
    );
    assert!(
        !system_text(&plain).contains(" — "),
        "不带声明的会话不该有任何索引行：{}",
        system_text(&plain)
    );
}

/// 139 重写（原名 `activating_one_skill_injects_only_its_own_body_and_tools`）：
/// 模型 `srv:skill/read` 其中一个 → **那一跳**的 tool_result 带上它的正文；
/// **另一个的正文全程不出现**（read 不改 `Slot::SkillsActive`）。140 起
/// `declaration()` 已经不带任何 skill 自带工具（那样的声明现在会 400），所以
/// 「自带工具不进表」不再是这条要测的事——那是声明时那道闸的职责。
///
/// 「另一个不出现」是这条的正对照：只断言「读的那个有了」的话，一个「把所有正文
/// 都注入」的实现同样会绿——而那正是延迟加载要避免的。
#[tokio::test(flavor = "multi_thread")]
async fn reading_one_skill_returns_only_its_own_body() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(read_call("crm-flow")),
        Script::Immediate(support::wire::text_reply("读完了。")),
    ]);
    let addr = start(&upstream).await;
    create(
        addr,
        json!({ "id": "declared", "capabilities": declaration() }),
    );

    let before = upstream.request_count();
    input(addr, "declared");
    wait_for_turn(&upstream, addr, "declared", before + 2).await;

    let after_read = body_at(&upstream, before + 1);

    // ── 读的那个：正文进这一跳的 tool_result（对话消息），不进 system 段。
    let tool_result = tool_result_text(&after_read, "call_a");
    assert!(
        tool_result.contains(CRM_BODY),
        "read 的 tool_result 该带上它的正文：{tool_result}"
    );
    assert!(
        !system_text(&after_read).contains(CRM_BODY),
        "139 起正文只在 tool_result 里，system 段不该出现它：{}",
        system_text(&after_read)
    );

    // ── 没读的那个：正文全程不出现，索引行还在。
    assert!(
        !tool_result.contains(MAIL_BODY) && !system_text(&after_read).contains(MAIL_BODY),
        "没读的那个不该有正文：tool_result={tool_result} system={}",
        system_text(&after_read)
    );
    assert!(
        system_text(&after_read).contains("mail-flow — 发信的标准流程"),
        "它的索引行该还在：{}",
        system_text(&after_read)
    );
}

/// 请求体里那条 `tool_call_id == call_id` 的 `role: "tool"` 消息的正文。
fn tool_result_text(body: &Value, call_id: &str) -> String {
    body["messages"]
        .as_array()
        .expect("请求体里该有 messages")
        .iter()
        .find(|m| m["role"] == json!("tool") && m["tool_call_id"] == json!(call_id))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
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
/// DeepSeek 只有一条常驻 system 消息（139 起没有激活式正文拼接这回事了），
/// 全程看的都是这一条。
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

/// 一段 DeepSeek 形状的流式回复：模型调 `srv:skill/read({"skill": "<id>"})`。
fn read_call(id: &str) -> String {
    let args = serde_json::to_string(&json!({ "skill": id }).to_string()).expect("json string");
    format!(
        concat!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":null,",
            "\"tool_calls\":[{{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",",
            "\"function\":{{\"name\":\"srv_3Askill_2Fread\",\"arguments\":{args}}}}}]}}}}]}}\n\n",
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"\"}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        args = args
    )
}
