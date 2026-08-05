//! 假 SSE 服务器（手写 `TcpListener`，零第三方 HTTP 依赖）验证：
//! - 流式回调按行收到数据，EOF 正常收尾
//! - 取消标志置位能打断一个「服务端不再发数据」的阻塞 read（issue 022 的硬骨头）
//! - 非 200 响应不重试（无论是 402 还是别的状态码）
//! - 断网（连接不上的本地端口）报明确错误，不是 panic、不是无限重试
//!
//! 手写 HTTP/1.1：只读到 `\r\n\r\n` + `Content-Length` 指定的请求体字节数，
//! 响应用 `Connection: close` + 直接断连表示 body 结束（ureq 按这个语义把
//! `into_reader()` 读到 EOF，见 crate 内部对 `BodyType::CloseDelimited` 的依赖）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use agent_transport::{Backoff, Client, StreamOutcome, TransportError};

/// 读一个请求到「空行」为止，再按 `Content-Length` 吃掉请求体，不解析别的。
fn drain_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return; // 对端提前断开
        }
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
}

/// 写一段「立刻可用」的 200 响应头，`Connection: close` 表示 body 读到 EOF 为止。
fn write_sse_headers(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    stream.flush().unwrap();
}

fn write_status(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn local_client() -> Client {
    Client::with_config(
        Duration::from_secs(5),
        // 短「取消轮询间隔」（`read_loop::run` 里 `recv_timeout` 的参数），
        // 测试要快——这个值只影响取消标志被发现的延迟，跟 socket 的读超时
        // （固定 60s，不给外部调，见 client.rs `DEFAULT_SOCKET_TIMEOUT`）无关。
        Duration::from_millis(120),
        Backoff {
            base: Duration::from_millis(30),
            max_attempts: 3,
        },
    )
}

/// 正常路径：几行 SSE 数据，中间夹一次短暂延迟（逼真：跨越至少一次「取消轮询
/// 间隔」，证明主流程在没有新行时反复 `recv_timeout` 空转、读线程仍在同一个
/// 连接上等下一行），随后正常关闭连接。回调按序收到全部行，`Finished` 收尾。
#[test]
fn streams_lines_and_finishes_on_clean_close() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        drain_request(&mut stream);
        write_sse_headers(&mut stream);
        stream.write_all(b"data: {\"choices\":[]}\n\n").unwrap();
        stream.flush().unwrap();
        // 跨过至少一次 120ms 的取消轮询间隔，证明没有新行时还能继续读同一个连接。
        std::thread::sleep(Duration::from_millis(260));
        stream.write_all(b"data: [DONE]\n\n").unwrap();
        stream.flush().unwrap();
        // drop(stream) 关闭连接 → 客户端读到 EOF。
    });

    let client = local_client();
    let cancel = AtomicBool::new(false);
    let mut lines = Vec::new();
    let outcome = client
        .post_stream(
            &format!("http://127.0.0.1:{port}/chat/completions"),
            "fake-key",
            b"{}",
            &cancel,
            |line| {
                if !line.is_empty() {
                    lines.push(line.to_string());
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();

    server.join().unwrap();
    assert_eq!(outcome, StreamOutcome::Finished);
    assert_eq!(lines, vec!["data: {\"choices\":[]}", "data: [DONE]"]);
}

/// 真实事故复现（023）：状态行（响应头）姗姗来迟——服务端接了连接、读完请求，
/// 但直到 700ms 之后才开始写响应头。022 的老实现把 500ms 的 socket 读超时同时
/// 套在「等状态行」阶段，这里会被误判成「连接失败」，退避重试 3 次后
/// `TransportError::Connect` 报「连接失败（尝试 3 次）」——这正是 Kimi 首字节
/// 慢触发的真实报错。023 把 socket 读超时放宽到 60s（`DEFAULT_SOCKET_TIMEOUT`，
/// 不受 `local_client()` 的取消轮询间隔影响），700ms 的延迟应该被稳稳等过去，
/// 只发生一次连接、拿到完整响应，不报任何错误。
#[test]
fn slow_status_line_is_tolerated_not_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicU32::new(0));
    let accepted_counter = Arc::clone(&accepted);

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        accepted_counter.fetch_add(1, Ordering::Relaxed);
        drain_request(&mut stream);
        // 慢首字节：接了请求，但迟迟不写响应头——这是 022 老实现（500ms 读
        // 超时）扛不住、023 要修的那个真实场景。
        std::thread::sleep(Duration::from_millis(700));
        write_sse_headers(&mut stream);
        stream.write_all(b"data: [DONE]\n\n").unwrap();
        stream.flush().unwrap();
    });

    let client = local_client();
    let cancel = AtomicBool::new(false);
    let mut lines = Vec::new();
    let start = Instant::now();
    let outcome = client
        .post_stream(
            &format!("http://127.0.0.1:{port}/chat/completions"),
            "fake-key",
            b"{}",
            &cancel,
            |line| {
                if !line.is_empty() {
                    lines.push(line.to_string());
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();
    let elapsed = start.elapsed();

    server.join().unwrap();
    assert_eq!(outcome, StreamOutcome::Finished);
    assert_eq!(lines, vec!["data: [DONE]"]);
    assert_eq!(
        accepted.load(Ordering::Relaxed),
        1,
        "慢首字节不该触发第二次连接（退避重试）"
    );
    assert!(
        elapsed >= Duration::from_millis(700),
        "该老老实实等了慢首字节，实际 {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "不该额外卡在退避里，实际 {elapsed:?}"
    );
}

/// 取消标志在流中途置位：服务端发一行后就不再发任何东西、也不关闭连接
/// （模拟「服务端还在，但没数据」）。客户端必须在若干个 poll 间隔内醒来发现
/// 取消标志，而不是一直阻塞到服务端最终关闭——这就是 issue 022 说的硬骨头。
#[test]
fn cancel_flag_interrupts_a_stalled_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        drain_request(&mut stream);
        write_sse_headers(&mut stream);
        stream.write_all(b"data: {\"choices\":[]}\n\n").unwrap();
        stream.flush().unwrap();
        // 故意长时间不发数据、不关闭——如果客户端没有短超时轮询，这里会把它
        // 一直卡住到这个 sleep 结束。
        std::thread::sleep(Duration::from_secs(5));
        let _ = stream.write_all(b"data: [DONE]\n\n");
    });

    let client = local_client();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_setter = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel_setter.store(true, Ordering::Relaxed);
    });

    let start = Instant::now();
    let outcome = client
        .post_stream(
            &format!("http://127.0.0.1:{port}/chat/completions"),
            "fake-key",
            b"{}",
            &cancel,
            |_| ControlFlow::Continue(()),
        )
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(outcome, StreamOutcome::Cancelled);
    assert!(
        elapsed < Duration::from_secs(2),
        "取消标志置位后该在几个 poll 间隔内醒来，实际等了 {elapsed:?}（服务端会卡 5s）"
    );
    // 不等服务端线程 join——它还卡在 5s 的 sleep 里，测试到这就已经验证完了
    // 客户端行为；进程退出时后台线程自然收场。
    drop(server);
}

/// `on_line` 回调自己返回 `Break`：也应该立刻停止读，不必等取消标志。
#[test]
fn on_line_break_stops_the_stream_without_cancel_flag() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        drain_request(&mut stream);
        write_sse_headers(&mut stream);
        stream.write_all(b"data: first\n\n").unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_secs(5));
        let _ = stream.write_all(b"data: second\n\n");
    });

    let client = local_client();
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let start = Instant::now();
    let outcome = client
        .post_stream(
            &format!("http://127.0.0.1:{port}/chat/completions"),
            "fake-key",
            b"{}",
            &cancel,
            |line| {
                if !line.is_empty() {
                    seen.push(line.to_string());
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();

    assert_eq!(outcome, StreamOutcome::Cancelled);
    assert_eq!(seen, vec!["data: first"]);
    assert!(start.elapsed() < Duration::from_secs(2));
}

/// 402 立刻报，不退避：断言只发生了一次连接（无论客户端算法本身，直接数
/// 服务端收到的连接次数），且耗时远小于「重试了才会等的」退避时长。
#[test]
fn payment_required_is_reported_without_retrying() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicU32::new(0));
    let accepted_counter = Arc::clone(&accepted);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline && !stop_flag.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    accepted_counter.fetch_add(1, Ordering::Relaxed);
                    stream.set_nonblocking(false).unwrap();
                    drain_request(&mut stream);
                    write_status(
                        &mut stream,
                        402,
                        "Payment Required",
                        r#"{"error":{"message":"Insufficient Balance"}}"#,
                    );
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    });

    // 退避基数拉大：如果客户端真的重试了，这次调用会花掉数百毫秒以上；
    // 如果没重试，应该在一次 RTT 内返回。
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(120),
        Backoff {
            base: Duration::from_secs(2),
            max_attempts: 3,
        },
    );
    let cancel = AtomicBool::new(false);
    let start = Instant::now();
    let err = client
        .post_stream(
            &format!("http://127.0.0.1:{port}/chat/completions"),
            "fake-key",
            b"{}",
            &cancel,
            |_| ControlFlow::Continue(()),
        )
        .unwrap_err();
    let elapsed = start.elapsed();

    assert_eq!(
        err,
        TransportError::Http {
            status: 402,
            body: r#"{"error":{"message":"Insufficient Balance"}}"#.to_string()
        }
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "402 不该退避，实际耗时 {elapsed:?}"
    );

    // 给服务端线程一点时间把这次连接计数落好，再关它。
    std::thread::sleep(Duration::from_millis(100));
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    assert_eq!(
        accepted.load(Ordering::Relaxed),
        1,
        "402 不该产生第二次连接"
    );
}

/// 断网：本地一个没人监听的端口。必须在有限次数内报明确错误，
/// 不是 panic、不是无限重试。
#[test]
fn unreachable_port_reports_a_bounded_connect_error() {
    // 拿一个当下空闲的端口号，随即把监听关掉——之后这个端口就是「连不上」。
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let client = Client::with_config(
        Duration::from_millis(300),
        Duration::from_millis(120),
        Backoff {
            base: Duration::from_millis(20),
            max_attempts: 3,
        },
    );
    let cancel = AtomicBool::new(false);
    let start = Instant::now();
    let err = client
        .post_stream(
            &format!("http://127.0.0.1:{port}/chat/completions"),
            "fake-key",
            b"{}",
            &cancel,
            |_| ControlFlow::Continue(()),
        )
        .unwrap_err();
    let elapsed = start.elapsed();

    match err {
        TransportError::Connect { attempts, .. } => assert_eq!(attempts, 3),
        other => panic!("期望 Connect 错误，拿到 {other:?}"),
    }
    // 3 次尝试 + 退避（20ms、40ms 量级）应该在很短时间内报完，不是卡死。
    assert!(
        elapsed < Duration::from_secs(5),
        "耗时 {elapsed:?}，看起来像在无限重试"
    );
}
