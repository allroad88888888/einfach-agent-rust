//! 钉住 [`crate::redacted_error`]：上游文本装进 `TransportError` 之后不含 api_key。
//!
//! # 这条测试为什么长这样（125 复核时重写过一次）
//!
//! 第一版是：测试自己调一遍 `redact`、自己构造 `TransportError`、再断言不含 key。
//! 那证明的是「`redact` 这个函数管用」——而 `redact` 本来就有自己的测试。
//! 真正要防的回归是**调用点漏掉遮罩**，那一版对它完全无感：把
//! `post_json_async` 里的 `redact(...)` 删掉，第一版照样绿。
//!
//! 现在钉的是 `redacted_error::{connect, http}` 这两个**调用点实际会用到的
//! 函数**：喂进去带 key 的原文，断言出来的 `TransportError` 的 `Display` 与
//! `Debug` 都不含 key。删掉 `redacted_error` 里的 `redact`，这里立刻红。
//!
//! # 仍然测不到的那一半（如实登记）
//!
//! `post_json_async` 本身要 `fetch`/`web_sys`，只在 wasm32 目标编译，
//! `cargo test --workspace` 跑在 native 上碰不到它。所以「`post_json_async`
//! 确实调用了 `redacted_error` 而不是就地构造」这一步**没有测试覆盖**，
//! 靠的是 `fetch_client.rs` 里那句注释 + code review。要把它也变成结构性的，
//! 得给 `TransportError` 的字段套 newtype，代价大于收益（见 `redacted_error`
//! 模块文档 §「这不是类型级保证，是收窄」）。

use crate::redacted_error;
use crate::TransportError;

const API_KEY: &str = "sk-post-json-async-test-key-must-not-leak";

#[test]
fn http_error_body_is_redacted_by_the_constructor() {
    // 401 的响应体回显 Authorization 头是真实存在的形态，不是杜撰的输入。
    let leaky_body = format!(r#"{{"error":"rejected Bearer {API_KEY}"}}"#);
    let error = redacted_error::http(401, &leaky_body, API_KEY);
    assert_key_not_leaked(&error);
    assert!(
        matches!(error, TransportError::Http { status: 401, .. }),
        "状态码必须原样带出去（分类归调用方，见 lib.rs「错误分类不在这里」）"
    );
}

#[test]
fn connect_error_message_is_redacted_by_the_constructor() {
    let leaky_message = format!("fetch 失败：Authorization Bearer {API_KEY} 被拒绝");
    let error = redacted_error::connect(1, &leaky_message, API_KEY);
    assert_key_not_leaked(&error);
}

/// 空 key 时 `redact` 原样返回（`upload.rs` 的既有行为）。这里钉一条，
/// 是为了防止有人把「空 key 不替换」误改成「空 key 替换成 `[REDACTED]`」
/// ——那会把每一条错误消息都洗成一串 `[REDACTED]`，排查时什么都看不到。
#[test]
fn an_empty_api_key_leaves_the_message_intact() {
    let message = "普通的连接失败，没有任何 key";
    let error = redacted_error::connect(3, message, "");
    assert!(
        format!("{error}").contains(message),
        "空 key 不该改动原文: {error}"
    );
}

fn assert_key_not_leaked(error: &TransportError) {
    assert!(
        !format!("{error}").contains(API_KEY),
        "Display 输出不得泄露 API key: {error}"
    );
    assert!(
        !format!("{error:?}").contains(API_KEY),
        "Debug 输出不得泄露 API key: {error:?}"
    );
}
