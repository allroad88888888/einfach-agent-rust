//! 062 的核心验收，**端到端、同一个 server 进程**：宿主在 `POST /sessions` 声明的
//! 工具只进**那一个 chatid** 的工具表。
//!
//! 断言的落点是**假上游收到的请求体**，不是某个内部结构：工具表最终的用处就是变成
//! prompt 字节发给模型，所以「进没进这个会话的表」这件事只有在这里才算真的证到。
//! 三个会话跑在同一个 `AgentServer`、同一份 `SessionTemplate` 上：
//!
//! | chatid | 声明 | 该看到什么 |
//! |---|---|---|
//! | `plain` | 无 | 只有部署期那两个内置工具（基线） |
//! | `declared` | 两个工具（**故意乱序给**） | 基线原封不动 + 两个注入的排在表尾、按名字排序 |
//! | `shuffled` | 同两个工具、又换一种顺序 | `tools` 段与 `declared` **逐字节相同** |
//!
//! 「作用域隔离」在这里是可判定的：`plain` 的 prompt 里**没有** `web:crm/lookup`
//! （HOST-CAPABILITIES.md §二）。红线 11 的两条也在这里：基线那一段一个字节不动
//! （§六 第 1 条）、客户端数组顺序进不了 prompt（§六 第 2 条；063 把后者钉到会红）。

mod support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use support::http_client;
use support::server::{FakeServer, Script};

/// 两个工具，**故意不按字典序给**（`web:` 在前、`desk:` 在后）：表里必须是反过来的。
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

/// 同样两个工具，另一种顺序——同一个会话表该长得一模一样。
fn declaration_shuffled() -> Value {
    let Value::Object(mut map) = declaration() else { unreachable!() };
    let Some(Value::Array(tools)) = map.get_mut("tools") else { unreachable!() };
    tools.reverse();
    Value::Object(map)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_only_reaches_the_session_that_declared_it() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let template = support::http_server::session_template(upstream.endpoint());
    let server = support::http_server::start_at_with_template("127.0.0.1:0".parse().unwrap(), template, |c| c).await;

    // 三个会话开在同一个进程、同一份 template 上。**先跑不带声明的那个**，它的
    // `tools` 段就是基线。
    create(server.addr, json!({ "id": "plain" }));
    create(server.addr, json!({ "id": "declared", "capabilities": declaration() }));
    create(server.addr, json!({ "id": "shuffled", "capabilities": declaration_shuffled() }));

    let baseline = tools_sent_by(&upstream, server.addr, "plain").await;
    let declared = tools_sent_by(&upstream, server.addr, "declared").await;
    let shuffled = tools_sent_by(&upstream, server.addr, "shuffled").await;

    // ── 基线：部署期那一档（`ToolTableSpec::Builtin`），一件不多。
    assert_eq!(names(&baseline), vec!["srv:fs/read", "srv:fs/list"], "不带声明的会话该跟 062 之前一模一样");

    // ── 作用域隔离（本 issue 最重要的一条）：另一个会话声明的工具，这里看不见。
    assert!(
        !names(&baseline).iter().any(|n| n.starts_with("web:") || n.starts_with("desk:")),
        "不带声明的会话的表里不该出现任何注入的工具：{baseline:#?}"
    );

    // ── 注入排在表尾：前 N 项与基线**逐项相同**（不只是名字，整个 JSON 对象）。
    assert_eq!(&declared[..baseline.len()], &baseline[..], "所有会话共有的那一段一个字节都不许动（红线 11）");
    assert_eq!(names(&declared)[baseline.len()..], ["desk:clipboard/write", "web:crm/lookup"], "注入的排表尾、按名字排序");

    // ── 声明的内容真的进了 prompt（不是只进了个名字）。
    let lookup = declared.last().expect("表尾就是它");
    assert_eq!(lookup["function"]["description"], json!("按客户 ID 查 CRM 档案"));
    assert_eq!(lookup["function"]["parameters"]["properties"]["id"]["type"], json!("string"));

    // ── 客户端数组顺序进不了 prompt：换个顺序声明，`tools` 段逐字节相同。
    assert_eq!(
        serde_json::to_string(&shuffled).unwrap(),
        serde_json::to_string(&declared).unwrap(),
        "同一份声明换个数组顺序，prompt 字节必须一模一样（红线 11）"
    );
}

fn create(addr: SocketAddr, body: Value) {
    let response = http_client::request(addr, "POST", "/sessions", Some(&body.to_string()));
    assert_eq!(response.status, 201, "{}", response.body);
}

/// 给这个会话发一句话，等假上游收到那一次 provider 调用，把请求体里的 `tools`
/// 数组取出来。会话是串行跑的（每次都等到请求真的到了才返回），所以「第几条请求体
/// 属于谁」是确定的。
async fn tools_sent_by(upstream: &FakeServer, addr: SocketAddr, id: &str) -> Vec<Value> {
    let before = upstream.request_count();
    let input = http_client::request(addr, "POST", &format!("/sessions/{id}/input"), Some(r#"{"text":"你好"}"#));
    assert_eq!(input.status, 202, "{}", input.body);

    let deadline = Instant::now() + Duration::from_secs(5);
    while upstream.request_count() == before {
        assert!(Instant::now() < deadline, "{id}：等假上游收到 provider 调用超时");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let body = upstream.bodies().swap_remove(before);
    let body: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{id} 的请求体不是 JSON：{e}\n{body}"));
    body["tools"].as_array().cloned().unwrap_or_default()
}

/// wire 上的 `function.name` 是转义过的（050：`srv:fs/read` → `srv_3Afs_2Fread`，
/// OpenAI 系的字符集不收冒号斜杠）——这里用 provider 自己那把解码器还原回全名，
/// 断言才读得懂，也不必把转义规则在测试里抄一遍。
fn names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .map(|t| agent_providers::wire_name::from_wire(t["function"]["name"].as_str().unwrap_or_default()).to_string())
        .collect()
}
