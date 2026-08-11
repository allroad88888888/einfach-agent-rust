//! **117 验收第二条的端到端那一半**（另一半是 `provider_call` 里那条确定性单
//! 测 `a_delta_already_in_the_channel_is_dropped_once_its_credential_is_gone`）：
//! 一条**真的走过 socket** 的晚到增量，在泵已经放弃那次 attempt 之后回来，必须
//! 找不到凭据而被丢掉，且一个字都不进消息历史。
//!
//! # 时序（两条测试共用一份服务器脚本，这是关键）
//!
//! 服务器第一条连接：写完响应头之后**闷 700ms**，然后吐一条文本增量「幽灵增量」。
//!
//! - [`a_late_delta_is_visible_when_its_attempt_is_still_in_flight`]（对照组）：
//!   provider 超时预算拉到 5s，凭据一直在飞 → 这条增量**看得见**（事件流里有它、
//!   消息历史里有它）。这一条证明的是「这份脚本、这条 socket、这个客户端配置下，
//!   那些字节确实会到达泵」——没有它，下面那条就可能只是「压根没收到」而绿。
//! - [`a_late_delta_from_an_abandoned_attempt_never_reaches_the_history`]（对抗
//!   组）：只把超时预算改成 500ms，别的一模一样 → 700ms 时凭据早已被
//!   `deadline::sweep` 划掉、重试已经开在飞（**同一个 epoch**，红线 6 的 epoch
//!   闸在这里帮不上忙），那条增量回到泵时只剩 `(agent, attempt)` 能分辨它。断言
//!   它没有变成事件、没有进历史，而且整轮仍然靠重试正常收工。
//!
//! # 一个必须说明的客户端配置
//!
//! `cancel_poll_interval` 特意拉到 10s。`deadline::sweep` 划掉凭据时会锁存该调用
//! 的取消标志，transport 的读循环按这个节奏轮询它——用默认的 50ms，被放弃的那条
//! 流会在 550ms 左右就被自己切断，700ms 的幽灵字节根本读不到，对抗组就变成一条
//! 空测试。拉长它是为了**让幽灵真的有机会回来**，不是为了掩盖什么。
//!
//! 顺带记一笔真实行为：读循环在每次读到一行之后才回头看取消标志，所以被放弃的
//! 那条流实际会**放过第一条晚到的行**（正是这里的幽灵），再判取消收场。换句话
//! 说这个窗口在 native 上不是构造出来的，是真的存在。

use crate::support;
use std::cell::RefCell;
use std::io::Write;
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use agent_core::{AgentId, ContentBlock, Session, SessionConfig, TurnStatus};
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{RunnerCtx, RunnerEvent, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};

/// 幽灵那一条增量的内容。它同时是「服务端真的写出去了」和「客户端没让它进历史」
/// 两条断言共用的记号。
const GHOST: &str = "幽灵增量";
/// 第一条连接闷多久再吐幽灵。
///
/// 三个时刻的关系是这条测试的全部：**超时预算 < 幽灵 < 整轮结束**。
/// `PROVIDER_BUDGET`(500ms) 到点 → 凭据被划掉、重试起飞；`GHOST_AFTER`(700ms)
/// 幽灵才回来（比划掉晚 200ms）；重试连接闷 `RETRY_AFTER`(400ms) 才答，所以整
/// 轮要到 900ms 之后才收工（比幽灵晚 200ms）。两头都留了 200ms 的余量，同时
/// `RETRY_AFTER < PROVIDER_BUDGET` 保证重试自己不会也被判超时。
const GHOST_AFTER: Duration = Duration::from_millis(700);
const PROVIDER_BUDGET: Duration = Duration::from_millis(500);
const RETRY_AFTER: Duration = Duration::from_millis(400);

struct Script {
    delay: Duration,
    lines: Vec<String>,
    /// 这条脚本把行写完之后置位——「服务端确实写出去了」的证据。
    written: Arc<AtomicBool>,
}

fn text_frame(text: &str) -> String {
    format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
    )
}

fn terminal_frames() -> Vec<String> {
    vec![
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#
            .to_string(),
        "data: [DONE]".to_string(),
    ]
}

/// 第 N 条连接用第 N 条脚本，**每条连接一个线程**（不能串行：对抗组里第二条
/// 连接必须在第一条还闷着的时候就被服务）。
fn spawn_server(scripts: Vec<Script>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for script in scripts {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            std::thread::spawn(move || {
                support::drain_request(&mut stream);
                support::write_sse_headers(&mut stream);
                std::thread::sleep(script.delay);
                for line in &script.lines {
                    let _ = stream.write_all(line.as_bytes());
                    let _ = stream.write_all(b"\n");
                }
                let _ = stream.flush();
                script.written.store(true, Ordering::Release);
            });
        }
    });
    port
}

/// 跟 `support::build_ctx` 同款装配，只有 `cancel_poll_interval` 不同（见模块
/// 文档最后一节），所以这里自己搭一份而不是给 support 加旋钮。
fn build_ctx(port: u16, root: &std::path::Path) -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_secs(10),
        Backoff {
            base: Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        ToolExecutor::new(root).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(None, |_| {}),
        Box::new(move |ev| sink.borrow_mut().push(ev)),
    );
    (ctx, events)
}

fn ghost_script(written: &Arc<AtomicBool>) -> Script {
    let mut lines = vec![text_frame(GHOST)];
    lines.extend(terminal_frames());
    Script {
        delay: GHOST_AFTER,
        lines,
        written: Arc::clone(written),
    }
}

fn saw_ghost_delta(events: &[RunnerEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, RunnerEvent::TextDelta(t) if t.contains(GHOST)))
}

fn history_mentions_ghost(session: &Session) -> bool {
    session.messages().iter().any(|m| {
        m.blocks.iter().any(|b| match b {
            ContentBlock::Text(t) => t.contains(GHOST),
            ContentBlock::ToolResult { content, .. } => content.contains(GHOST),
            _ => false,
        })
    })
}

/// 对照组：凭据还在飞的时候，同一份脚本的那条增量**看得见**。
#[test]
fn a_late_delta_is_visible_when_its_attempt_is_still_in_flight() {
    let dir = support::temp_dir("late-delta-control");
    let written = Arc::new(AtomicBool::new(false));
    let port = spawn_server(vec![ghost_script(&written)]);
    let (ctx, events) = build_ctx(port, &dir);
    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(5));
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::run_turn(&mut session, &mut ctx, "你好")
        .expect("a timed-out provider attempt is not a transient-source failure");

    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert!(
        written.load(Ordering::Acquire),
        "服务端该把幽灵那一行写出去"
    );
    assert!(
        saw_ghost_delta(&events.borrow()),
        "凭据还在飞时这条增量必须看得见——看不见的话下面那条对抗测试就是空的：{:#?}",
        events.borrow()
    );
    assert!(
        history_mentions_ghost(&session),
        "凭据还在飞时它也该进消息历史：{:#?}",
        session.messages()
    );
}

/// 对抗组：只把超时预算改小，同一份字节从一次**已被放弃的 attempt** 上回来。
#[test]
fn a_late_delta_from_an_abandoned_attempt_never_reaches_the_history() {
    let dir = support::temp_dir("late-delta-ghost");
    let written = Arc::new(AtomicBool::new(false));
    let mut retry_lines = vec![text_frame("重试的答案")];
    retry_lines.extend(terminal_frames());
    let port = spawn_server(vec![
        // 第 1 条连接：500ms 时被判超时并放弃，700ms 才吐出幽灵。
        ghost_script(&written),
        // 第 2 条连接：重试。故意答得比幽灵晚，保证泵在幽灵回来的那一刻还活着。
        Script {
            delay: RETRY_AFTER,
            lines: retry_lines,
            written: Arc::new(AtomicBool::new(false)),
        },
    ]);
    let (ctx, events) = build_ctx(port, &dir);
    let mut ctx = ctx.with_provider_timeout(PROVIDER_BUDGET);
    let mut session = Session::new(AgentId::root());
    session.set_max_retries(1);

    let started = Instant::now();
    let status = agent_runtime::run_turn(&mut session, &mut ctx, "你好")
        .expect("a timed-out provider attempt is not a transient-source failure");
    let elapsed = started.elapsed();

    // 幽灵确实被写出去了，而且泵在那之后还活着——否则这条测试什么都没测到。
    assert!(
        written.load(Ordering::Acquire),
        "服务端该把幽灵那一行写出去"
    );
    assert!(
        elapsed > GHOST_AFTER,
        "整轮必须活过幽灵回来的那一刻（{GHOST_AFTER:?}），实际只有 {elapsed:?}"
    );

    // 重试正常收工：被放弃那次的终态（取消）没有冒领重试的凭据。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "被放弃的 attempt 的终态一旦冒领了重试的凭据，这一轮会变成 Failed(Cancelled)"
    );
    let reply = session.messages().last().cloned().expect("该有一条回复");
    assert!(
        matches!(&reply.blocks[0], ContentBlock::Text(t) if t.contains("重试的答案")),
        "最终答案该来自重试那一次：{reply:#?}"
    );

    // 幽灵没有留下任何痕迹（红线 6 的同款判据：结果确实回来了 + 历史里没有它）。
    assert!(
        !saw_ghost_delta(&events.borrow()),
        "被放弃的 attempt 的增量不该发给宿主：{:#?}",
        events.borrow()
    );
    assert!(
        !history_mentions_ghost(&session),
        "幽灵增量进了消息历史——`(agent, attempt)` 没挡住它：{:#?}",
        session.messages()
    );
}
