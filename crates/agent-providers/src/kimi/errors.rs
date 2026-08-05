//! Kimi 错误分类：直接用共享骨架（`crate::wire::errors`）。三条实测都落在
//! 通用的 `error.type` 关键词匹配里，不需要 Kimi 专属规则（PROVIDERS.md §四）：
//!
//! - 404 `resource_not_found_error`（模型名错）→ 关键词 `not_found` → `BadRequest`
//!   （不掉进「认不出的 404 = Unknown」——404 在别处通常意味着不可恢复的路径
//!   问题，但这里靠 type 才知道语义其实是「请求本身不对」）
//! - 429 `engine_overloaded_error` → 关键词 `overload` → `Retryable`
//! - 401 `invalid_authentication_error` → 关键词 `auth` → `Auth`

use agent_core::ErrorClass;

use crate::wire;

pub(crate) fn classify(status: u16, body: &str) -> ErrorClass {
    wire::errors::classify(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_name_error_is_404_but_classified_bad_request() {
        let body =
            r#"{"error":{"message":"The model does not exist","type":"resource_not_found_error"}}"#;
        assert_eq!(classify(404, body), ErrorClass::BadRequest);
    }

    #[test]
    fn overload_is_429_classified_retryable() {
        let body = r#"{"error":{"message":"engine overloaded, try again later","type":"engine_overloaded_error"}}"#;
        assert_eq!(classify(429, body), ErrorClass::Retryable);
    }

    #[test]
    fn invalid_key_is_401_classified_auth() {
        let body =
            r#"{"error":{"message":"invalid api key","type":"invalid_authentication_error"}}"#;
        assert_eq!(classify(401, body), ErrorClass::Auth);
    }

    /// 指定函数与思考模式互斥的那个 400，走通用的 `invalid_request` 关键词。
    #[test]
    fn tool_choice_thinking_conflict_is_bad_request() {
        let body = r#"{"error":{"message":"tool_choice 'specified' is incompatible with thinking enabled","type":"invalid_request_error"}}"#;
        assert_eq!(classify(400, body), ErrorClass::BadRequest);
    }
}
