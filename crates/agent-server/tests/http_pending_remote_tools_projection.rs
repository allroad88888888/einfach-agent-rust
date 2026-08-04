//! 072：**待办投影**——`GET /sessions/{id}/pending_tools` 导出此刻还欠着的远端调用。
//!
//! 三条，各钉一件事：
//!
//! 1. **病因证据**（今天就绿，不是修复目标）：同一个 chatid 上换一条**没有游标**的
//!    新连接，那帧 `tool_executing` **原样又来一次**。这把「爆炸半径在 chatid 那条
//!    路上」钉成可执行的事实，而不是文档里的一句话；顺带守住「我们没去偷偷改写
//!    ring」（候选 1 被否的直接理由——ring 必须还是日志）。**SSE 和 `/events/poll`
//!    各断一次**：网关走的是后者，只测 SSE 等于没测正主。
//! 2. **投影跟槽同生同灭**：`ToolExecuting` 之后精确断言只有那一条，`POST
//!    /tool_result` 之后立刻为空。
//! 3. **超时那一路同款**：`take_expired_remote_tools`（060 的截止线）也得让投影
//!    收缩——漏了这一处，前端刷新后会去执行一个**已经按失败收尾**的调用，回传照旧
//!    被安全拒绝、副作用照旧发生，正是本 issue 的病换个入口复发。
//!
//! 建会话时**指定 chatid**（`POST /sessions {"id":"chat-072"}`，055 的幂等
//! getOrCreate）而不是「模拟刷新页面」：demo 每次开页新建会话，刷新 = 新 session =
//! 空 ring，压根碰不到这条 bug（issue §现象 的 ⚠️ 块）。判据是「会话身份被复用 +
//! 客户端没有游标」。

mod support;

use std::time::Duration;

use agent_server::{Frame, SessionEvent, ToolTableSpec};

use support::http_client;
use support::server::{FakeServer, Script};

const CHAT_ID: &str = "chat-072";

fn browser_action_reply() -> String {
    [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_browser_1","type":"function","function":{"name":"browser_action","arguments":"{\"action\":\"render_card\",\"payload\":{\"title\":\"Hello\"}}"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
        "",
    ]
    .join("\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> Frame {
    let raw = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data))
}

/// 起服务器 + 建**指定 chatid** 的会话 + 连一条 SSE + 喂一句输入，一直读到那条
/// `ToolExecuting`。返回 `(server, sse, agent, call_id)`。
async fn dispatch_a_web_tool(remote_tool_timeout: Option<Duration>) -> (support::http_server::TestServer, http_client::SseReader, String, String) {
    let upstream = FakeServer::start(vec![
        Script::Immediate(browser_action_reply()),
        Script::Immediate(support::wire::text_reply("已渲染。")),
    ]);
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.tools = ToolTableSpec::Standard;
    template.remote_tool_timeout = remote_tool_timeout;
    let server = support::http_server::start_at_with_template("127.0.0.1:0".parse().unwrap(), template, |config| config).await;
    // 假上游得活到本条测试结束——它是 `FakeServer`（后台线程），泄漏掉即可，
    // 跟既有的 web 工具测试同一个取舍。
    std::mem::forget(upstream);

    let create = http_client::request(server.addr, "POST", "/sessions", Some(&format!("{{\"id\":\"{CHAT_ID}\"}}")));
    assert_eq!(create.status, 201, "{}", create.body);
    let (status, _, mut sse) = http_client::connect_sse(server.addr, &format!("/sessions/{CHAT_ID}/events"), None);
    assert_eq!(status, 200);

    let input = http_client::request(server.addr, "POST", &format!("/sessions/{CHAT_ID}/input"), Some("{\"text\":\"展示卡片\"}"));
    assert_eq!(input.status, 202, "{}", input.body);

    let (agent, call_id) = loop {
        let frame = next_frame(&mut sse);
        if let SessionEvent::ToolExecuting { call_id, request } = frame.event {
            assert_eq!(&*request.tool, "browser_action");
            break (frame.agent.0.to_string(), call_id.0.to_string());
        }
    };
    (server, sse, agent, call_id)
}

fn pending_tools(server: &support::http_server::TestServer) -> String {
    let res = http_client::request(server.addr, "GET", &format!("/sessions/{CHAT_ID}/pending_tools"), None);
    assert_eq!(res.status, 200, "{}", res.body);
    res.body
}

fn post_result(server: &support::http_server::TestServer, agent: &str, call_id: &str) {
    let body = serde_json::json!({ "agent": agent, "tool_call_id": call_id, "result": { "content": "{\"cardId\":\"card-1\"}" } }).to_string();
    let res = http_client::request(server.addr, "POST", &format!("/sessions/{CHAT_ID}/tool_result"), Some(&body));
    assert_eq!(res.status, 202, "{}", res.body);
}

/// 读到本轮落终态为止——收场之后再去开新连接，才是「历史里那条早就干完的活」。
fn drain_until_terminal(sse: &mut http_client::SseReader) {
    loop {
        let frame = next_frame(sse);
        if matches!(&frame.event, SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) if status.is_terminal()) {
            return;
        }
    }
}

/// **病因证据**：这条今天就该绿——它证明的是 bug 存在的前提（同 chatid + 无游标 =
/// 整个 ring 重放），不是修复目标。修完之后它必须**继续**绿：ring 是日志，
/// 谁都不许为了「别让前端再执行一次」去改写它（候选 1）。
#[tokio::test(flavor = "multi_thread")]
async fn a_cursorless_client_on_the_same_chatid_gets_the_settled_tool_executing_frame_again() {
    let (server, mut sse, agent, call_id) = dispatch_a_web_tool(None).await;
    post_result(&server, &agent, &call_id);
    drain_until_terminal(&mut sse);

    // 换一条**不带 `Last-Event-ID`** 的新 SSE 连接：浏览器刷新 / 新 tab / 网关重启
    // 在服务端看来长得一模一样（`parseCursor(null)` → `replay(None)`）。
    let (status, _, mut replayed) = http_client::connect_sse(server.addr, &format!("/sessions/{CHAT_ID}/events"), None);
    assert_eq!(status, 200);
    let mut saw_executing = false;
    while let Some(raw) = replayed.next_event(Duration::from_millis(500)) {
        let frame: Frame = serde_json::from_str(&raw.data).unwrap_or_else(|e| panic!("{e}: {}", raw.data));
        if matches!(&frame.event, SessionEvent::ToolExecuting { request, .. } if &*request.tool == "browser_action") {
            // `--nocapture` 时把证据本身打出来：这一帧跟第一次派活时那一帧是同一条，
            // 客户端拿不到任何「我已经干过了」的信息（对它而言，这就是一次新派发）。
            println!("[072 病因证据] 无游标的新 SSE 连接补发到的帧：id={:?} {}", raw.id, raw.data);
            saw_executing = true;
        }
    }
    assert!(saw_executing, "无游标的新 SSE 连接该拿到整个 ring 的重放，其中含那条早已收场的 tool_executing——这就是 072 的爆炸半径");

    // **网关走的是这一条**（M9 拉取式）：`/events/poll` 不带 `Last-Event-ID`
    // 同样是全量重放。只测 SSE 等于没测正主。
    let poll = http_client::request(server.addr, "GET", &format!("/sessions/{CHAT_ID}/events/poll"), None);
    assert_eq!(poll.status, 200, "{}", poll.body);
    println!("[072 病因证据] 无游标的 /events/poll 拿到的整批：{}", poll.body);
    assert!(
        poll.body.contains("tool_executing") && poll.body.contains("browser_action"),
        "无游标的 poll 同样该拿到那条 tool_executing（网关背后的浏览器就是这么中招的）：{}",
        poll.body
    );
}

/// 投影跟等待槽**同生同灭**：派出去就有，回传收场就没。
#[tokio::test(flavor = "multi_thread")]
async fn the_projection_holds_exactly_the_waiting_call_and_empties_on_the_result() {
    let (server, mut sse, agent, call_id) = dispatch_a_web_tool(None).await;

    let listing = pending_tools(&server);
    assert!(listing.contains(&call_id), "派出去的调用该在投影里：{listing}");
    assert!(listing.contains(&agent), "投影要带 agent 归属（回传要按 (agent, call_id) 精确匹配）：{listing}");
    assert!(listing.contains("browser_action"), "投影要带 request，前端据此执行：{listing}");
    assert_eq!(listing.matches("call_id").count(), 1, "只该有这一条：{listing}");

    post_result(&server, &agent, &call_id);
    drain_until_terminal(&mut sse);

    let after = pending_tools(&server);
    assert!(!after.contains(&call_id), "回传收场之后投影里那一条该立刻没了：{after}");
    assert!(!after.contains("browser_action"), "投影必须跟着 take_remote_tool 收缩：{after}");
}

/// 第四个变更点：`discard_remote_tools`（`POST /cancel`，undo/redo 走同一个函数）。
/// 取消斩断的槽同样已经收场，投影漏了它就是同一个病换第三个入口。
#[tokio::test(flavor = "multi_thread")]
async fn the_projection_empties_when_the_turn_is_cancelled() {
    let (server, _sse, _agent, call_id) = dispatch_a_web_tool(None).await;
    assert!(pending_tools(&server).contains(&call_id), "刚派出去，投影里该有它");

    let cancel = http_client::request(server.addr, "POST", &format!("/sessions/{CHAT_ID}/cancel"), None);
    assert_eq!(cancel.status, 202, "{}", cancel.body);

    // 取消是 fire-and-forget（202 不等 actor 处理完），给 actor 一小段时间把
    // `Command::Cancel` 收下来——投影的写点在 `discard_remote_tools` 那一刻，
    // 不在这次 HTTP 响应上。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && pending_tools(&server).contains(&call_id) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let after = pending_tools(&server);
    assert!(!after.contains(&call_id), "取消斩断等待槽之后投影必须同步收缩：{after}");
}

/// 060 的截止线那一路同款：`take_expired_remote_tools` 也是四个变更点之一，漏了
/// 它，前端刷新后会去执行一个**已经按失败收尾**的调用。
#[tokio::test(flavor = "multi_thread")]
async fn the_projection_empties_when_the_deadline_takes_the_slot() {
    let (server, mut sse, _agent, call_id) = dispatch_a_web_tool(Some(Duration::from_millis(300))).await;
    assert!(pending_tools(&server).contains(&call_id), "刚派出去，投影里该有它");

    // 这一段**故意什么都不做**：没有回传、没有取消，只等 060 的截止线到点。
    drain_until_terminal(&mut sse);

    let after = pending_tools(&server);
    assert!(!after.contains(&call_id), "截止线取走槽之后投影必须同步收缩：{after}");
    assert!(!after.contains("browser_action"), "否则前端刷新后会执行一个已经按失败收尾的调用：{after}");
}
