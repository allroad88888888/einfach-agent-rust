//! 独立测试覆盖点 5：Cancel 斩全树。
//!
//! root 一跳吐三个并行 spawn，三个子的连接全部在飞（脚本全部挂住不回）。
//! 200ms 后置位取消标志，`run_turn` 该在几个 poll 间隔内落
//! `Failed(Cancelled)`——不是靠恰好撞上某个超时预算（预算特意设得远大于
//! 这条测试的时间尺度，跟 `cancel.rs` 的既有手法一致）。
//!
//! 这里不用 `tests/support/routed.rs`（它按「服务完才记一条 `Call`」计数，
//! 三个挂住的子连接要等脚本自己的长睡眠走完才会入账，量不出「取消之后
//! 有没有新连接」）。自己写一个只关心「接进来了几条连接」的最小服务器
//! （029 任务说明允许「假服务器手法可抄 routed.rs 或自写」）：每接进一条
//! 连接立刻给计数器加一，第一条按 root 的首跳正常应答，其余全部挂住不回——
//! 这样「取消之后还有没有新连接」量的是接入计数，不受挂住连接本身要多久
//! 才算「服务完」影响。

mod spawn_indep_support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_core::{AgentId, AgentLimits, Failure, Session, TurnStatus};
use agent_runtime::run_turn;

use spawn_indep_support::{build_ctx, sse_tool_calls, temp_dir, wire_tool_name};

fn drain(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
}

/// 每接进一条连接立刻计数；第一条正常应答 root 的首跳（三个并行 spawn），
/// 其余的写完响应头就长睡眠——不回、也不断，模拟三个子全部在飞。
fn spawn_accept_counting_server(hop1_lines: Vec<String>) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_bg = Arc::clone(&accepted);
    thread::spawn(move || {
        for (i, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { return };
            let seq = accepted_bg.fetch_add(1, Ordering::SeqCst);
            let hop1_lines = hop1_lines.clone();
            thread::spawn(move || {
                drain(&mut stream);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                let _ = stream.flush();
                if i == 0 {
                    for line in &hop1_lines {
                        let _ = stream.write_all(line.as_bytes());
                        let _ = stream.write_all(b"\n");
                    }
                    let _ = stream.flush();
                } else {
                    // 三个子（以及任何本不该出现的重试）全部挂在这里，
                    // 时间尺度远超这条测试关心的窗口。
                    thread::sleep(Duration::from_secs(20));
                }
                let _ = seq;
            });
        }
    });
    (port, accepted)
}

#[test]
fn cancel_mid_flight_cuts_every_child_and_the_server_sees_no_further_connections() {
    let dir = temp_dir("cancel-tree");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let hop1 = sse_tool_calls(&[
        (
            "call_x",
            &spawn_wire,
            r#"{"task":"HANGX first hanging child"}"#,
        ),
        (
            "call_y",
            &spawn_wire,
            r#"{"task":"HANGY second hanging child"}"#,
        ),
        (
            "call_z",
            &spawn_wire,
            r#"{"task":"HANGZ third hanging child"}"#,
        ),
    ]);
    let (port, accepted) = spawn_accept_counting_server(hop1);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (ctx, _events) = build_ctx(port, &dir, tools);
    // 超时预算远大于这条测试的时间尺度：观察到的终态必须是取消标志起的
    // 作用，不是我们自己的超时机制抢跑撞上同一个终态巧合看起来一样。
    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(10));

    let cancel = ctx.cancel_flag();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    let mut session = Session::new(AgentId::root());
    let start = Instant::now();
    let status = run_turn(
        &mut session,
        &mut ctx,
        "cancelkick spawn three hanging workers",
    );
    let elapsed = start.elapsed();

    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert!(
        elapsed < Duration::from_secs(2) && elapsed >= Duration::from_millis(200),
        "该在置位之后的几个 poll 间隔内收尾，不该等到 10s 的超时预算或 20s 的挂住时长，实际 {elapsed:?}"
    );
    assert!(
        session.tool_slots().is_empty(),
        "取消要把 root 的工具槽全部弃掉"
    );

    // root 首跳 + 三个子，四条连接该都已经接进来了（它们是取消发生*之前*
    // 就已经在飞的，取消斩断的是它们的结果，不是不让它们发生）。
    let accepted_at_return = accepted.load(Ordering::SeqCst);
    assert_eq!(
        accepted_at_return, 4,
        "取消发生前该已经有 1 个 root 首跳 + 3 个子连接"
    );

    // 收工后再等一段：不该有第五条连接进来——没有重试，没有孤儿线程继续
    // 敲服务器。
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        accepted_at_return,
        "取消之后不该再有任何新连接"
    );
}
