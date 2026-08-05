//! DeepSeek 错误分类：判定机制在 `crate::wire::errors`（三家共用，见那边的
//! 模块文档）。这个文件只留这家的录制回归——真实响应体不会变，逻辑挪了地方
//! 不代表行为可以漂移。

use agent_core::ErrorClass;

use crate::wire;

pub(crate) fn classify(status: u16, body: &str) -> ErrorClass {
    wire::errors::classify(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 录制的三个错误体（wire-shape.json）：401 走 type，两个 400 走 type。
    #[test]
    fn recorded_bodies() {
        let key = r#"{"error":{"code":"invalid_request_error","message":"Authentication Fails, Your api key: ****robe is invalid","param":null,"type":"authentication_error"}}"#;
        assert_eq!(classify(401, key), ErrorClass::Auth);

        let model = r#"{"error":{"code":"invalid_request_error","message":"The supported API model names are ...","param":null,"type":"invalid_request_error"}}"#;
        assert_eq!(classify(400, model), ErrorClass::BadRequest);

        // 裸壳（没有 error 包一层）也要认。
        let choice = r#"{"code":"invalid_request_error","message":"Thinking mode does not support this tool_choice","param":null,"type":"invalid_request_error"}"#;
        assert_eq!(classify(400, choice), ErrorClass::BadRequest);
    }

    /// 402 单列：**不管响应体怎么写**都是 `Exhausted`。
    #[test]
    fn payment_required_is_always_exhausted() {
        let ds = r#"{"error":{"message":"Insufficient Balance","type":"unknown_error","code":"invalid_request_error"}}"#;
        assert_eq!(classify(402, ds), ErrorClass::Exhausted);
        assert_eq!(classify(402, ""), ErrorClass::Exhausted);
        assert_eq!(classify(200, ds), ErrorClass::Exhausted);
    }

    #[test]
    fn status_fallback_covers_the_five_buckets() {
        assert_eq!(classify(401, ""), ErrorClass::Auth);
        assert_eq!(classify(400, ""), ErrorClass::BadRequest);
        assert_eq!(classify(429, ""), ErrorClass::Retryable);
        assert_eq!(classify(503, ""), ErrorClass::Retryable);
        assert_eq!(classify(500, "not json at all"), ErrorClass::Retryable);
        assert_eq!(
            classify(599, r#"{"error":{"type":"mystery"}}"#),
            ErrorClass::Retryable
        );
        assert_eq!(classify(404, ""), ErrorClass::Unknown);
        assert_eq!(
            classify(418, r#"{"error":{"type":"teapot"}}"#),
            ErrorClass::Unknown
        );
    }
}
