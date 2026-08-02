//! GLM 错误分类：直接用共享骨架（`crate::wire::errors`）。PROVIDERS.md §四
//! 没有给出 GLM 错误体的具体 `type` 取值（模型名错误 400、鉴权失败 401，跟
//! DeepSeek 的状态码分配一致），所以这里主要靠状态码兜底路径覆盖，`type`
//! 匹配到就更准（跟另两家共用同一套关键词，不需要 GLM 专属规则）。

use agent_core::ErrorClass;

use crate::wire;

pub(crate) fn classify(status: u16, body: &str) -> ErrorClass {
    wire::errors::classify(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模型名不存在：GLM 给 400（不是 Kimi 的 404），没有已知 `type`，靠状态码
    /// 兜底判成 `BadRequest`。
    #[test]
    fn model_name_error_is_400_bad_request() {
        assert_eq!(classify(400, r#"{"error":{"message":"model not found"}}"#), ErrorClass::BadRequest);
    }

    /// key 无效：401，靠状态码兜底判成 `Auth`。
    #[test]
    fn invalid_key_is_401_auth() {
        assert_eq!(classify(401, r#"{"error":{"message":"invalid api key"}}"#), ErrorClass::Auth);
    }

    /// 如果这家的 `type` 恰好带上通用关键词，一样能被共享逻辑判对——不需要
    /// GLM 专属分支。
    #[test]
    fn generic_type_keywords_still_match_when_present() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"too many requests"}}"#;
        assert_eq!(classify(429, body), ErrorClass::Retryable);
    }
}
