//! `classify(status, body)`：HTTP 状态 + 响应体 → `ErrorClass`（PROVIDERS.md §四）。
//!
//! 骨架取自 probes/results/wire-shape.json：`{"error": {"message","type",...}}`，
//! DeepSeek 额外带 `code`/`param`。400/401 两条直接照抄 wire-shape.json 里
//! DeepSeek 的真实记录（`error.bad_param` / `error.bad_key`）；429/402/503/599
//! 没有被 probes 录到（402 要真的把账户刷穷，429/503 要真的把服务打过载，
//! 没人会为了录一条错误帧去干这个），按 PROVIDERS.md §四已经写死的骨架
//! （`error.type`/`code`/`param`/`message` 四个键）构造，值参考同一节的速查表：
//! DeepSeek 过载是 503，402 是余额耗尽，两条都是 DeepSeek 独有的分配。
//!
//! **402 必须单列成 `Exhausted`**——PROVIDERS.md 原话：混进限流会安静地退避到
//! 天荒地老。

mod support;

use agent_core::ErrorClass;
use agent_providers::Provider;

#[test]
fn status_401_classifies_as_auth() {
    let provider = support::provider();
    // 逐字取自 probes/results/wire-shape.json deepseek.error.bad_key。
    let body = r#"{"error": {"code": "invalid_request_error", "message": "Authentication Fails, Your api key: ****robe is invalid", "param": null, "type": "authentication_error"}}"#;
    assert_eq!(provider.classify(401, body), ErrorClass::Auth);
}

#[test]
fn status_400_classifies_as_bad_request() {
    let provider = support::provider();
    // 逐字取自 probes/results/wire-shape.json deepseek.error.bad_param。
    let body = r#"{"error": {"code": "invalid_request_error", "message": "Failed to deserialize the JSON body into the target type: max_tokens: invalid value: integer `-1`, expected u32 at line 1 column 16", "param": null, "type": "invalid_request_error"}}"#;
    assert_eq!(provider.classify(400, body), ErrorClass::BadRequest);
}

#[test]
fn status_402_classifies_as_exhausted_not_retryable() {
    let provider = support::provider();
    // PROVIDERS.md §四：402 是 DeepSeek 独有的「余额耗尽」。probes 没录到真实
    // 响应体（要求账户真的被刷穷），按同一节确认的骨架构造。
    let body = r#"{"error": {"code": "insufficient_quota", "message": "Insufficient Balance", "param": null, "type": "insufficient_quota_error"}}"#;
    assert_eq!(
        provider.classify(402, body),
        ErrorClass::Exhausted,
        "402 必须单列为 Exhausted，混进 Retryable 会让退避重试到天荒地老"
    );
}

#[test]
fn status_429_classifies_as_retryable() {
    let provider = support::provider();
    let body = r#"{"error": {"code": "rate_limit_error", "message": "rate limited", "param": null, "type": "rate_limit_error"}}"#;
    assert_eq!(provider.classify(429, body), ErrorClass::Retryable);
}

#[test]
fn status_503_classifies_as_retryable() {
    let provider = support::provider();
    // PROVIDERS.md §四：DeepSeek 的「过载」场景状态码正是 503。
    let body = r#"{"error": {"code": "engine_overloaded", "message": "service overloaded", "param": null, "type": "engine_overloaded_error"}}"#;
    assert_eq!(provider.classify(503, body), ErrorClass::Retryable);
}

#[test]
fn unrecognized_5xx_like_599_stays_retryable() {
    // 合并时裁决过的分歧：seam.rs 对 Retryable 的契约原话是「限流、过载、5xx」，
    // 599 在 5xx 段内——判成 Unknown 会把 522 这类中间层状态码变成不重试，
    // 那是实际损失。Unknown 用 5xx 段外的码测（下一个测试）。
    let provider = support::provider();
    let body = r#"{"error": {"code": null, "message": "mystery", "param": null, "type": "totally_unrecognized_error_type"}}"#;
    assert_eq!(provider.classify(599, body), ErrorClass::Retryable);
}

#[test]
fn unrecognized_non_5xx_status_classifies_as_unknown() {
    let provider = support::provider();
    // 302：不在任何已知分类段里（非 401/400/402/429/5xx），error.type 也认不出。
    let body = r#"{"error": {"code": null, "message": "mystery", "param": null, "type": "totally_unrecognized_error_type"}}"#;
    assert_eq!(provider.classify(302, body), ErrorClass::Unknown);
}
