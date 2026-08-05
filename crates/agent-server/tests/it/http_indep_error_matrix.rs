//! 031 独立测试 agent：错误码矩阵（issue 031 验收「不存在的 id → 404
//! `session_not_found`；dead 会话 → 410 `session_dead`；坏 JSON body → 400；
//! `tool_result` → 202/404（与其它会话命令一致）；`undo` step+force → 400」）。
//!
//! # 410（dead 会话）：如实记录未能独立复现
//!
//! 尝试过的黑盒触发路径：十种畸形假上游响应体（空 body、非法 JSON、空
//! `choices`、缺 `delta`、超大/负数 usage、多 `choices`、没有 `id` 的
//! `tool_calls`、参数不是 JSON 的 `tool_calls`）全部被优雅吸收成
//! `Failed(Provider(Unknown))` 或正常 `Done`，session 全程 `alive`——系统对
//! 畸形上游的健壮性看起来是刻意做到的，不是漏了 catch。另外试过
//! `session_path` 指向目录（拿到了 409 `session_conflict`，body IO 失败的
//! 映射）和指向父目录不存在的路径（201，正常 alive，父目录看起来是懒创建或
//! 干脆不需要预先存在）。`agent-server` 内部的 `SessionRegistry` 不通过
//! `AgentServer` 的公开面对外暴露，纯 HTTP 黑盒没有另一条「直接构造」的路径
//! 能把一个已标记 dead 的会话塞进 `AgentServer` 自己持有的那张表。410 的错误
//! 码**映射**逻辑（`session_dead` 字符串、状态码 410）在 issue 031 实做记录里
//! 提到有单元测试钉住，但那不是独测 agent 能验的东西（那是实现方代码的白盒
//! 覆盖）。如实记在这里，不假装覆盖到了。

mod http_indep_support;

use http_indep_support::fake_upstream::{FakeUpstream, Script};
use http_indep_support::raw_http::{post_json, request};
use http_indep_support::server_harness::{HarnessConfig, start};

async fn server() -> http_indep_support::server_harness::TestServer {
    let upstream = FakeUpstream::start(vec![Script::Text("hi".to_string())]);
    // upstream 得活到 server 用完为止——泄漏它（测试进程很快退出，OS 收）。
    std::mem::forget(upstream);
    start(
        "http://127.0.0.1:1/unused".to_string(), // 这一批测试都不需要真的成功跑一轮
        HarnessConfig::default(),
    )
    .await
}

fn assert_error_shape(body: &serde_json::Value, expected_code: &str) {
    let error = &body["error"];
    assert!(
        error.is_object(),
        "错误体该是 {{\"error\":{{...}}}} 形状，实际：{body}"
    );
    assert_eq!(
        error["code"], expected_code,
        "错误码不对，完整 body：{body}"
    );
    assert!(
        error["message"].is_string(),
        "message 该是字符串，完整 body：{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nonexistent_session_id_is_404_on_status_and_events() {
    let server = server().await;

    let status = request(server.addr, "GET", "/sessions/does-not-exist", &[], None);
    assert_eq!(status.status, 404);
    assert_error_shape(&status.json(), "session_not_found");

    let events = request(
        server.addr,
        "GET",
        "/sessions/does-not-exist/events",
        &[],
        None,
    );
    assert_eq!(
        events.status,
        404,
        "events 端点对不存在的 id 也该 404，实际 body={}",
        events.body_str()
    );
    assert_error_shape(&events.json(), "session_not_found");

    let input = post_json(
        server.addr,
        "/sessions/does-not-exist/input",
        "{\"text\":\"hi\"}",
    );
    assert_eq!(input.status, 404);
    assert_error_shape(&input.json(), "session_not_found");

    let undo = post_json(
        server.addr,
        "/sessions/does-not-exist/undo",
        "{\"granularity\":\"turn\",\"force\":false}",
    );
    assert_eq!(undo.status, 404);
    assert_error_shape(&undo.json(), "session_not_found");

    let redo = post_json(server.addr, "/sessions/does-not-exist/redo", "{}");
    assert_eq!(redo.status, 404);
    assert_error_shape(&redo.json(), "session_not_found");

    let cancel = post_json(server.addr, "/sessions/does-not-exist/cancel", "{}");
    assert_eq!(cancel.status, 404);
    assert_error_shape(&cancel.json(), "session_not_found");
}

/// 坏 JSON body 的状态码：两条路径都确认是 400（这部分符合验收）。body 的
/// **形状**是不是统一错误 JSON 见下面转正的
/// `malformed_json_body_uses_the_unified_error_shape`——那部分是分歧，如实
/// 分开钉。
#[tokio::test(flavor = "multi_thread")]
async fn malformed_json_body_is_400() {
    let server = server().await;
    let id = server.create_session();

    let bad = post_json(
        server.addr,
        &format!("/sessions/{id}/input"),
        "{not json at all",
    );
    assert_eq!(
        bad.status,
        400,
        "坏 JSON body 该 400，实际 body={}",
        bad.body_str()
    );

    let bad_undo = post_json(
        server.addr,
        &format!("/sessions/{id}/undo"),
        "not even an object",
    );
    assert_eq!(bad_undo.status, 400);
}

/// 分歧：issue 031「落地」小节原文「错误统一 JSON `{"error":{"code",
/// "message"}}`」——实测坏 JSON body 这条路径没有走到这套统一形状。axum 的
/// `Json<T>` extractor 在请求体反序列化失败时，会在 handler 函数**真正被
/// 调用之前**就短路返回它自己内置的 rejection 响应（`Content-Type:
/// text/plain; charset=utf-8`，body 是形如 `"Failed to parse the request
/// body as JSON: key must be a string at line 1 column 2"` 的纯文本），
/// 不会经过应用自己的错误映射逻辑——这是 axum 集成里一个有名的坑（要统一
/// 形状得自己写一个包装 extractor 顶替裸 `Json<T>`，或者在 `FromRequest`
/// 失败分支上再包一层）。状态码本身是对的（400），错、并且当前会一直失败
/// 的只是 body 形状（031 分歧修复后 rejection 已走统一错误形状，测试转正）。
#[tokio::test(flavor = "multi_thread")]
async fn malformed_json_body_uses_the_unified_error_shape() {
    let server = server().await;
    let id = server.create_session();
    let bad = post_json(
        server.addr,
        &format!("/sessions/{id}/input"),
        "{not json at all",
    );
    assert_eq!(
        bad.header("content-type"),
        Some("application/json"),
        "实际是 text/plain"
    );
    assert_error_shape(&bad.json(), "bad_request");
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_step_granularity_with_force_is_400() {
    let server = server().await;
    let id = server.create_session();

    // Session 没有 undo_step 的 force 变体（issue 031 实做记录），HTTP 层在
    // 转发给 actor 之前就该拒绝。
    let resp = server.post_undo(&id, "step", true);
    assert_eq!(
        resp.status,
        400,
        "step+force=true 该被拒，实际 body={}",
        resp.body_str()
    );
    assert_error_shape(&resp.json(), "bad_request");

    // 对照组：step + force=false、turn + force=true 都该正常放行（202）。
    let ok1 = server.post_undo(&id, "step", false);
    assert_eq!(
        ok1.status,
        202,
        "step+force=false 该正常放行，body={}",
        ok1.body_str()
    );
    let ok2 = server.post_undo(&id, "turn", true);
    assert_eq!(
        ok2.status,
        202,
        "turn+force=true 该正常放行，body={}",
        ok2.body_str()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_result_uses_the_same_session_lookup_as_other_commands() {
    let server = server().await;
    let id = server.create_session();

    let existing = server.post_tool_result(&id);
    assert_eq!(
        existing.status,
        202,
        "活 session 的回传应成功入 actor 队列，body={}",
        existing.body_str()
    );

    let missing = post_json(
        server.addr,
        "/sessions/does-not-exist/tool_result",
        "{\"agent\":\"root\",\"tool_call_id\":\"x\",\"result\":{\"content\":\"x\"}}",
    );
    assert_eq!(
        missing.status,
        404,
        "不存在的 session 不应接受回传，body={}",
        missing.body_str()
    );
    assert_error_shape(&missing.json(), "session_not_found");
}

/// 附带发现（不在原 8 条覆盖点里，顺手钉住）：`session_path` 指向一个已存在
/// 的目录而不是文件，会在打开会话文件时报 IO 错误，映射成 409
/// `session_conflict`——不是 500，也不是不受控的 panic。
#[tokio::test(flavor = "multi_thread")]
async fn session_path_pointing_at_a_directory_is_409_not_a_crash() {
    let server = server().await;
    let resp = post_json(server.addr, "/sessions", "{\"session_path\":\"/\"}");
    assert_eq!(resp.status, 409, "body={}", resp.body_str());
    assert_error_shape(&resp.json(), "session_conflict");
}
