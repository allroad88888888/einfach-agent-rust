//! 独立测试 agent 依据 156（docs/issues/156-server-prefix-declaration.md）「验收」
//! 一节 + HOST-CAPABILITIES §八之三 写的规格测试——不看 `http/capabilities/{validate_prefix,
//! capability_prefix,assemble}.rs`/`actor/capabilities.rs` 的实现，只按协议契约断言。
//!
//! 本文件管**装配正确性**这一件事：合法的 `capabilities.prefix` 声明，首轮
//! encode body 里到底长什么样。四条：
//! 1) 单块声明——文本逐字节原样进 system 段（含引号/换行/emoji，抓 JSON 转义）；
//! 2) 三块乱序声明 + 一个 skill——块序按 name 字节序，且排在内置索引块之后；
//! 3) `prefix` 与 `disable_builtin` 同一次声明里共存，两者互不干扰；
//! 4) `"prefix": []` 与完全不带 `capabilities`：首轮请求体逐字节相同（红线 11）。
//!
//! 判据统一是**假上游收到的请求体**，跟本仓既有的 062/064/076 系列测试同一种
//! 证明手法。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::AgentLimits;
use agent_server::{SessionTemplate, ToolTableSpec};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CHAT_ID: &str = "prefix-decl-indep";

/// 一段刻意带引号、换行、中文与 emoji 的文本——服务端把它转发进上游请求体时
/// 必须逐字节原样：一个「二次转义」或「反转义漏一层」的实现会在这里露馅
/// （转义后的样子跟原文截然不同，`.contains(text)` 直接抓不住）。
const ESCAPE_HEAVY_TEXT: &str =
    "今天的客户上下文：\n第一步说\"你好\"。\n😀🚀 完毕，别漏字节。";

/// **验收第 1 条**：单块声明 → 首轮 system 段含这块文本，逐字节原样。
#[tokio::test(flavor = "multi_thread")]
async fn single_block_text_round_trips_byte_exact_with_escaping() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start(&upstream).await;

    let declaration = json!({
        "prefix": [ { "name": "web:crm/briefing", "text": ESCAPE_HEAVY_TEXT } ]
    });
    create(addr, json!({ "id": CHAT_ID, "capabilities": declaration }), 201);

    let body = one_turn(&upstream, addr, CHAT_ID).await;
    let text = system_text(&body);
    assert!(
        text.contains(ESCAPE_HEAVY_TEXT),
        "声明的文本该逐字节原样出现在 system 段（引号/换行/emoji 都不能变样）：{text}"
    );
    // 装配是追加，不是替换——部署期那句基础 system 该还在。
    assert!(
        text.contains("test"),
        "宿主声明该是追加在基础 system 之后，不是把它换掉：{text}"
    );
}

/// **验收第 2 条**：三块乱序声明 → 块序 = name 字节序；且排在内置 init 块
/// （这里用 skill 索引当那个「内置块」）之后——156 验收原句「排在内置 init 块
/// （如 skills 索引）之后」。
///
/// 三个 name 故意选跨 `desk:`/`web:` 两种合法前缀，字节序上 `desk:` 打头的
/// 排最前（`d` < `w`），声明数组则反着给（zzz、aaa、mmm），逼实现真的排序
/// 而不是巧合般跟数组顺序一致。
#[tokio::test(flavor = "multi_thread")]
async fn three_blocks_declared_out_of_order_render_in_name_byte_order_after_skill_index() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start(&upstream).await;

    let declaration = json!({
        "skills": [
            { "id": "diag-flow", "description": "诊断标准流程", "body": "先看日志。" }
        ],
        "prefix": [
            { "name": "web:zzz/last", "text": "MARKER_ZZZ_LAST" },
            { "name": "desk:aaa/first", "text": "MARKER_AAA_FIRST" },
            { "name": "web:mmm/middle", "text": "MARKER_MMM_MIDDLE" }
        ]
    });
    create(addr, json!({ "id": CHAT_ID, "capabilities": declaration }), 201);

    let body = one_turn(&upstream, addr, CHAT_ID).await;
    let text = system_text(&body);

    let skill_index_pos = text
        .find("diag-flow — 诊断标准流程")
        .unwrap_or_else(|| panic!("system 段该含 skill 索引行：{text}"));
    let aaa = text
        .find("MARKER_AAA_FIRST")
        .unwrap_or_else(|| panic!("缺 desk:aaa/first 的块：{text}"));
    let mmm = text
        .find("MARKER_MMM_MIDDLE")
        .unwrap_or_else(|| panic!("缺 web:mmm/middle 的块：{text}"));
    let zzz = text
        .find("MARKER_ZZZ_LAST")
        .unwrap_or_else(|| panic!("缺 web:zzz/last 的块：{text}"));

    assert!(
        skill_index_pos < aaa && skill_index_pos < mmm && skill_index_pos < zzz,
        "宿主声明的前缀块该排在内置 init 块（skill 索引）之后：skill@{skill_index_pos} aaa@{aaa} mmm@{mmm} zzz@{zzz}\n{text}"
    );
    assert!(
        aaa < mmm && mmm < zzz,
        "三块该按 name 字节序排列（desk:aaa/first < web:mmm/middle < web:zzz/last），\
         不是声明数组给的顺序（zzz、aaa、mmm）：aaa@{aaa} mmm@{mmm} zzz@{zzz}\n{text}"
    );
}

/// **自选边界**：`prefix` 与 076 的 `disable_builtin` 同一次声明里一起用——一个
/// 只做加法、一个只做减法，理论上互不相干，但两者都是「同一份声明落 store」的
/// 一部分，值得钉一条「同时用不会互相打架」。
#[tokio::test(flavor = "multi_thread")]
async fn prefix_and_disable_builtin_declared_together_both_take_effect() {
    let upstream = FakeServer::start(vec![Script::Immediate(support::wire::text_reply("好的。"))]);
    let addr = start_full(&upstream).await;

    let declaration = json!({
        "disable_builtin": [ "srv:agent/spawn" ],
        "prefix": [ { "name": "web:combo/marker", "text": "COMBO_MARKER_TEXT_9F2Q" } ]
    });
    create(addr, json!({ "id": CHAT_ID, "capabilities": declaration }), 201);

    let body = one_turn(&upstream, addr, CHAT_ID).await;
    assert!(
        !names(&body).contains(&"srv:agent/spawn".to_string()),
        "disable_builtin 该照常生效，没被 prefix 的装配打断：{:?}",
        names(&body)
    );
    assert!(
        system_text(&body).contains("COMBO_MARKER_TEXT_9F2Q"),
        "prefix 该照常生效，没被 disable_builtin 的装配打断：{}",
        system_text(&body)
    );
}

/// **验收第 4 条**：`"capabilities": { "prefix": [] }` 与完全不带 `capabilities`
/// ——首轮请求体逐字节相同（红线 11：没声明就该跟没这个字段的世界一模一样）。
///
/// 比较整段原始请求体字符串（不是重新序列化的 `Value`），这是「逐字节」这个词
/// 能给到的最强证明——一个「多套了一层空对象再序列化」的实现会在字节层面露馅，
/// 即使反序列化成 `Value` 之后看着「相等」。
#[tokio::test(flavor = "multi_thread")]
async fn empty_prefix_array_and_omitted_capabilities_produce_byte_identical_first_request() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(support::wire::text_reply("好的。")),
        Script::Immediate(support::wire::text_reply("好的。")),
    ]);
    let addr = start(&upstream).await;

    create(
        addr,
        json!({ "id": "with-empty-prefix", "capabilities": { "prefix": [] } }),
        201,
    );
    create(addr, json!({ "id": "without-capabilities" }), 201);

    let before = upstream.request_count();
    input(addr, "with-empty-prefix");
    input(addr, "without-capabilities");
    wait_for(&upstream, before + 2).await;

    let bodies = upstream.bodies();
    let raw_a = bodies[before].clone();
    let raw_b = bodies[before + 1].clone();
    assert_eq!(
        raw_a, raw_b,
        "`prefix: []` 与完全不带 capabilities 的首轮请求体必须逐字节相同"
    );
}

async fn start(upstream: &FakeServer) -> SocketAddr {
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        support::http_server::session_template(upstream.endpoint()),
        |c| c,
    )
    .await;
    server.addr
}

/// 部署期开满档——`disable_builtin` 那条自选边界要能关掉 `srv:agent/spawn`，
/// `Builtin` 档没有它可关。
async fn start_full(upstream: &FakeServer) -> SocketAddr {
    let server =
        support::http_server::start_at_with_template("127.0.0.1:0".parse().unwrap(), full_template(upstream.endpoint()), |c| c)
            .await;
    server.addr
}

fn full_template(endpoint: String) -> SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.tools = ToolTableSpec::Full {
        spawn_limits: AgentLimits::default(),
    };
    template
}

fn create(addr: SocketAddr, body: Value, want: u16) {
    let response = http_client::request(addr, "POST", "/sessions", Some(&body.to_string()));
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
