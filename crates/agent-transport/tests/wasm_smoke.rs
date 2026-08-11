//! wasm32 目标的真实执行验证：不是 mock，是真的 `wasm-pack test --node`
//! 跑起来的 wasm 二进制，连 Node 的真实 `fetch`/`AbortController`/
//! `ReadableStream`，对着 `tests/wasm_node/server.mjs` 起的真实 HTTP
//! 服务器发请求——用 `crates/agent-transport/tests/wasm_node/run.sh` 跑。
//!
//! 只在 wasm32 目标编译：`wasm-bindgen-test` 这条 dev-dependency 挂在
//! `[target.'cfg(target_arch = "wasm32")'.dev-dependencies]` 下，
//! `cargo test -p agent-transport`（native）不会看到它——`#![cfg(...)]`
//! 这行让整个文件在其它目标上直接编译成空 crate，不会因为缺依赖报错。
//!
//! 场景对应 `tests/it/fake_sse.rs`（native 那条真实生产路径的验收）：干净
//! 关闭、402 不重试、abort 打断卡住的流——用同一组场景断言两条平台真实
//! 生产路径行为一致，是「分帧一致性」证明的第三条腿（前两条：
//! `src/framing_parity_tests.rs` 在 native 上直接比对 `drive_stream` 与
//! `read_loop::run`；`cargo check --target wasm32-unknown-unknown` 确认
//! `web_sys` 接线类型对得上）。
#![cfg(target_arch = "wasm32")]

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_transport::{Client, StreamOutcome, TransportError};
use wasm_bindgen_test::wasm_bindgen_test;

const BASE: &str = "http://127.0.0.1:18391";

#[wasm_bindgen_test]
async fn clean_close_streams_expected_lines() {
    let client = Client::new();
    let cancel = AtomicBool::new(false);
    let mut lines = Vec::new();
    let outcome = client
        .post_stream_async(
            &format!("{BASE}/clean-close"),
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
        .await
        .unwrap();

    assert_eq!(lines, vec!["data: {\"choices\":[]}", "data: [DONE]"]);
    assert_eq!(outcome, StreamOutcome::Finished);
}

#[wasm_bindgen_test]
async fn slow_first_byte_is_tolerated() {
    // 对应 native fake_sse.rs::slow_status_line_is_tolerated_not_retried——
    // fetch 没有 native 那种「短 socket 超时误判慢首字节」的问题（lib.rs
    // 顶部「wasm 侧」一节的正面印证：这一整类 bug 在 wasm 上不需要修，因为它
    // 根本不存在），这里只验证慢首字节仍然能正常收到完整响应。
    let client = Client::new();
    let cancel = AtomicBool::new(false);
    let mut lines = Vec::new();
    let outcome = client
        .post_stream_async(
            &format!("{BASE}/slow-first-byte"),
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
        .await
        .unwrap();

    assert_eq!(lines, vec!["data: [DONE]"]);
    assert_eq!(outcome, StreamOutcome::Finished);
}

#[wasm_bindgen_test]
async fn payment_required_is_reported_without_retrying() {
    let client = Client::new();
    let cancel = AtomicBool::new(false);
    let err = client
        .post_stream_async(
            &format!("{BASE}/payment-required"),
            "fake-key",
            b"{}",
            &cancel,
            |_| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();

    assert_eq!(
        err,
        TransportError::Http {
            status: 402,
            body: r#"{"error":{"message":"Insufficient Balance"}}"#.to_string(),
        }
    );
}

#[wasm_bindgen_test]
async fn abort_interrupts_a_stalled_stream() {
    // 对应 native fake_sse.rs::cancel_flag_interrupts_a_stalled_stream:
    // 服务端发一行后不再发数据、不关闭连接;取消标志置位后必须在有限时间内
    // 从「卡在一次读中间」里醒过来——这是 web_stream_source.rs 里
    // `race(reader.read(), wait_until_cancelled(...))` 那条路径专门要证明
    // 的场景,不是「两次成功读取之间查一眼」那种简单情况。
    let client = Client::with_config(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        agent_transport::Backoff {
            base: std::time::Duration::from_millis(20),
            max_attempts: 3,
        },
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_setter = cancel.clone();
    wasm_bindgen_futures::spawn_local(async move {
        gloo_free_delay(200).await;
        cancel_setter.store(true, Ordering::Relaxed);
    });

    let mut lines = Vec::new();
    let outcome = client
        .post_stream_async(
            &format!("{BASE}/stall-forever"),
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
        .await
        .unwrap();

    assert_eq!(lines, vec!["data: first"]);
    assert_eq!(outcome, StreamOutcome::Cancelled);
}

/// 不依赖 `gloo-timers`(避免只为一个测试文件新增一个 crate 依赖):用跟
/// 生产代码同款的「反射拿全局 setTimeout」手法包一个 `Promise`。
async fn gloo_free_delay(ms: i32) {
    use wasm_bindgen::{JsCast, JsValue};
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let global = js_sys::global();
        if let Ok(set_timeout) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            && let Ok(set_timeout) = set_timeout.dyn_into::<js_sys::Function>()
        {
            let _ = set_timeout.call2(&global, &resolve, &JsValue::from_f64(ms as f64));
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
