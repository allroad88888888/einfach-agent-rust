//! `McpRegistry` 的**锁粒度**（issue 070）：要串行的是「同一个 server」，不是
//! 「所有 server」。
//!
//! 两条断言互为对照，缺一条这个改动就是错的：
//!
//! 1. 慢 server 的一次往返**不挡住**别的 server 的调用（070 的全部意义）。
//!    修之前这条是红的——`with_client` 在整张表的锁里跑完整个 JSON-RPC 往返。
//! 2. 同一个 server 的两次并发调用**仍然不重叠**——它内部只有一条 stdio 管道，
//!    应答靠 `id` 匹配，交错就乱（`client` 模块文档「应答匹配」）。
//!
//! 假 server 照本 crate 既有手法用一段 `sh`（`handshake_translate_042.rs`）：`read`
//! 逐行吃请求、`printf` 逐行回响应，零网络零 npm。client 的 id 序列是确定的
//! （`initialize`=1，之后每个请求 +1，通知不占 id），所以脚本里把 `tools/call` 的
//! 响应 id 写死。

use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use agent_mcp::{McpClient, McpRegistry, flatten_tool_result};
use serde_json::json;

/// 往返超时给足——本文件测的是「谁等谁」，超时不该参与结论。
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

const HANDSHAKE: &str = r#"read init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}'
read initialized
"#;

/// 一个假 server：握手之后读一次 `tools/call`，`delay` 秒后回一条文本结果。
fn one_call_script(delay: &str, text: &str) -> String {
    format!(
        r#"{HANDSHAKE}read call
sleep {delay}
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":"{text}"}}]}}}}'
"#
    )
}

/// 一个假 server：握手之后**依次**读两次 `tools/call`，每次 `delay` 秒后回一条。
/// 两条结果的文本不同（`first`/`second`），用来验证应答没有串号。
fn two_call_script(delay: &str) -> String {
    format!(
        r#"{HANDSHAKE}read c1
sleep {delay}
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":"first"}}]}}}}'
read c2
sleep {delay}
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"content":[{{"type":"text","text":"second"}}]}}}}'
"#
    )
}

fn connect(script: &str) -> McpClient {
    McpClient::connect(
        "sh",
        &["-c".to_string(), script.to_string()],
        &[],
        "agent-mcp-concurrency-test",
        "0.0.0",
        Duration::from_secs(10),
    )
    .expect("假 server 该握手成功")
}

/// 一次 `tools/call` 的结果文本（`None` = server id 查不到，或者调用失败）。
fn call_text(registry: &McpRegistry, server: &str) -> Option<String> {
    let out = registry.with_client(server, |c| c.call("echo", json!({}), CALL_TIMEOUT))?;
    out.ok().map(|v| flatten_tool_result(&v).text)
}

/// **070 的核心**：`slow` 正在跑一次 1s 的往返时，对 `fast` 的调用不该排队等它。
///
/// 时序是真的、不靠 sleep 猜：慢调用进到 `with_client` 的闭包里之后**发一个信号**
/// （旧实现里那一刻整张表的锁已经被它攥住了），主线程收到信号才开始计时打 `fast`。
/// 于是「fast 有没有被挡住」被压成一个可判定的数字：它的耗时是几毫秒，还是慢
/// server 那 1 秒。
#[test]
fn a_slow_server_does_not_block_a_call_to_another_server() {
    let registry = Arc::new(McpRegistry::new());
    registry.insert("slow", connect(&one_call_script("1", "slow")));
    registry.insert("fast", connect(&one_call_script("0", "fast")));

    let (entered_tx, entered_rx) = mpsc::channel();
    let slow_registry = Arc::clone(&registry);
    let slow_thread = thread::spawn(move || {
        let started = Instant::now();
        let text = slow_registry.with_client("slow", |c| {
            // 已经进到闭包里 = 旧实现里整张表的锁已经在手上。
            entered_tx.send(()).expect("主线程还在等这个信号");
            c.call("echo", json!({}), CALL_TIMEOUT)
        });
        let text = text
            .expect("slow 该在表里")
            .ok()
            .map(|v| flatten_tool_result(&v).text);
        (started.elapsed(), text)
    });

    entered_rx
        .recv()
        .expect("慢调用该进到 with_client 的闭包里");

    let started = Instant::now();
    let fast = call_text(&registry, "fast");
    let fast_elapsed = started.elapsed();

    let (slow_elapsed, slow) = slow_thread.join().expect("慢调用线程不该 panic");

    assert_eq!(slow.as_deref(), Some("slow"), "慢 server 的调用本身该成功");
    assert_eq!(fast.as_deref(), Some("fast"), "快 server 的调用本身该成功");
    assert!(
        slow_elapsed >= Duration::from_millis(900),
        "慢 server 得真的慢，否则这条测试是空的；实际 {slow_elapsed:?}"
    );
    assert!(
        fast_elapsed < Duration::from_millis(300),
        "对 fast 的调用不该等 slow 的往返（issue 070）；fast 花了 {fast_elapsed:?}，\
         slow 花了 {slow_elapsed:?}"
    );
}

/// 反向护栏：同一个 server 的两次并发调用**仍然串行**。
///
/// 断言用两段真实区间：每个线程在闭包内记下进入/离开的时刻，两段区间必须**不相交**。
/// 假 server 每次调用要 0.3s，所以区间都是非退化的——真让两次往返同时在飞的话
/// （一条管道两份请求先后写进去、响应按到达顺序回），两段区间会重叠，这条就红。
/// 再加一条内容断言：两个线程各拿到一条、且是不同的那条，证明应答没串号。
#[test]
fn two_concurrent_calls_to_the_same_server_stay_serialized() {
    let registry = Arc::new(McpRegistry::new());
    registry.insert("one", connect(&two_call_script("0.3")));

    let gate = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let registry = Arc::clone(&registry);
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                gate.wait();
                registry
                    .with_client("one", |c| {
                        let entered = Instant::now();
                        let out = c.call("echo", json!({}), CALL_TIMEOUT);
                        (
                            entered,
                            Instant::now(),
                            out.ok().map(|v| flatten_tool_result(&v).text),
                        )
                    })
                    .expect("one 该在表里")
            })
        })
        .collect();

    let mut spans: Vec<(Instant, Instant, Option<String>)> = threads
        .into_iter()
        .map(|t| t.join().expect("调用线程不该 panic"))
        .collect();
    spans.sort_by_key(|(entered, ..)| *entered);

    let (a_in, a_out, a_text) = spans[0].clone();
    let (b_in, b_out, b_text) = spans[1].clone();

    let mut texts = vec![
        a_text.expect("先跑的那次该成功"),
        b_text.expect("后跑的那次该成功"),
    ];
    texts.sort();
    assert_eq!(
        texts,
        vec!["first".to_string(), "second".to_string()],
        "两次应答不该串号"
    );

    assert!(
        a_out - a_in >= Duration::from_millis(250) && b_out - b_in >= Duration::from_millis(250),
        "每次往返都该真的花掉假 server 的 0.3s，否则区间退化、下面的不相交断言是空的"
    );
    assert!(
        a_out <= b_in,
        "同一个 server 的两次调用必须不重叠（一条 stdio 管道）；\
         第一段 {:?}，第二段在第一段结束前 {:?} 就开始了",
        a_out - a_in,
        a_out - b_in
    );
}
