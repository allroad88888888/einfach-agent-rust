//! 集成测试共用的假 SSE 服务器 + `OpenSpec` 装配 + 「收事件收到终态」的小工具。
#![allow(dead_code)]

pub mod http_client;
pub mod http_server;
pub mod routed;
pub mod server;
pub mod wire;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_core::{Notice, SystemChunk, TurnStatus};
use agent_providers::deepseek::DeepSeek;
use agent_server::{Frame, OpenSpec, SessionEvent, SessionId, Subscription, ToolTableSpec};
use agent_transport::{Backoff, Client};

/// 每个用例一个独立临时目录，不清理——OS/CI 环境自行回收（跟
/// `agent-runtime/tests/support::temp_dir` 同一个取舍）。
pub fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-server-it-{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 指向假服务器的 `OpenSpec`：DeepSeek adapter（三家里已经有录制帧验证过 wire
/// 形状的那家）、短连接超时/取消轮询节奏（测试要快）。`store_path` 由调用方
/// 决定：`None` = 临时会话，`Some` = 落盘（供 close/reopen 恢复测试用）。
pub fn open_spec(id: &str, endpoint: String, store_path: Option<PathBuf>) -> OpenSpec {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    OpenSpec {
        id: SessionId::from(id),
        store_path,
        provider: Arc::new(DeepSeek),
        endpoint,
        api_key: "fake-key".to_string(),
        model: Arc::from("deepseek-v4-pro"),
        tools: ToolTableSpec::Builtin,
        tools_root: temp_dir(&format!("tools-{id}")),
        system: vec![SystemChunk { label: Arc::from("base"), text: Arc::from("test") }],
        client: Arc::new(client),
        history_cap: None,
        snapshot_every: Some(0), // 关掉节奏噪音——恢复靠 entry 重放，快照不是这些测试关心的事。
        provider_timeout: Some(Duration::from_secs(5)),
    }
}

/// 反复 `recv`，攒到一条 `Notice::TurnStatusChanged { status }` 且
/// `status.is_terminal()` 为真为止（这条本身也进返回的 `Vec` 里）。超过
/// `budget` 收不到就直接 panic——测试不该无限期挂着。
///
/// 034：`Subscription::recv` 给的是 [`Frame`]（agent 归属信封），这里**拆掉
/// 信封只留 `event`**——绝大多数既有测试断言的是「这一轮发生了什么」，不关心
/// 「谁发的」，拆包让它们一行不用改。真要断言归属的测试用
/// [`collect_frames_until_terminal`]。
pub async fn collect_until_terminal(sub: &mut Subscription, budget: Duration) -> Vec<SessionEvent> {
    collect_frames_until_terminal(sub, budget).await.into_iter().map(|frame| frame.event).collect()
}

/// 同 [`collect_until_terminal`]，但不拆信封——给需要断言 `frame.agent` 的测试
/// 用（034 验收：spawn 轮经 HTTP 之后两个子 agent 的归属交错出现）。
pub async fn collect_frames_until_terminal(sub: &mut Subscription, budget: Duration) -> Vec<Frame> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, sub.recv())
            .await
            .unwrap_or_else(|_| panic!("等终态事件超时，已收到 {out:?}"))
            .unwrap_or_else(|| panic!("事件流提前结束（session 没了），已收到 {out:?}"));
        let terminal = matches!(&frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal());
        out.push(frame);
        if terminal {
            return out;
        }
    }
}

/// 从一批事件里把 `TextDelta` 拼成一个字符串——断言「这轮回复的文本是什么」
/// 时比逐条 `matches!` 短。
pub fn text_of(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|ev| match ev {
            SessionEvent::TextDelta(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect()
}

/// 最后一条事件是不是终态 `TurnStatus`，方便 `assert!(matches!(..))` 前先拿到。
pub fn terminal_status(events: &[SessionEvent]) -> Option<TurnStatus> {
    events.iter().rev().find_map(|ev| match ev {
        SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal() => Some(status.clone()),
        _ => None,
    })
}

/// 极简 JSON 字符串字段提取——031 的 HTTP 测试（`POST /sessions` 的
/// `{"id":"..."}`、状态查询的 `{"status":"alive"}` 这类小响应体）不值得为了
/// 断言拉一个完整 JSON 解析依赖；`SessionEvent` 那种复杂帧走
/// `serde_json::from_str` 反序列化成真类型，不用这个。
pub fn extract_json_string_field(body: &str, field: &str) -> String {
    let needle = format!("\"{field}\":\"");
    let Some(start) = body.find(&needle) else { return String::new() };
    let rest = &body[start + needle.len()..];
    let end = rest.find('"').unwrap_or(rest.len());
    rest[..end].to_string()
}
