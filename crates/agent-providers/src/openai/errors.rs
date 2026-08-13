//! 通用 OpenAI 兼容端点的错误分类。
//!
//! 骨架用共享的 `crate::wire::errors`（先按 `error.type` 判，落不到按状态码兜底）。
//! **这里比三家更需要状态码兜底**，因为 174 实测出兼容端点的错误体形状差得离谱：
//!
//! | 端点 | 模型名不存在时 |
//! |---|---|
//! | DeepSeek | `400` + `{"error":{"type":"invalid_request_error",…}}` |
//! | Kimi | **`404`** + `{"error":{"type":"resource_not_found_error",…}}` |
//! | GLM | `400` + `{"error":{"code":"1214","message":"modelCode：不存在"}}` — **没有 `type`** |
//! | GLM（路径打错时） | `404` + `{"timestamp":…,"status":404,"error":"Not Found","path":…}` — **整个不是 OpenAI 形状** |
//!
//! 所以这里的纪律是：**认不出就落 `Unknown`，不许猜**。`Unknown` 的语义是
//! 「保守处理、不自动重试」，猜错的代价是要么把一次性失败无限重试（把 `BadRequest`
//! 猜成 `Retryable`），要么把一次限流当成永久失败（反过来猜）——两个都比多问一次人贵。
//!
//! **裸 404 特别值得说**：`by_status` 里没有 404 这一档，所以它落 `Unknown`。
//! 对通用 adapter 而言这恰好是对的——收到裸 404 + 非 OpenAI 错误体，最可能的
//! 真相是**用户的 `base_url` 填错了**（174 里就是这么撞出来的：给 GLM 拼了个
//! `/v1`，整组 404）。那是配置错误，要人去看，不是「这次请求内容不合法」。
//! 而带 OpenAI 形状的 404（Kimi 的模型名不存在）会经 `error.type` 判成
//! `BadRequest`。**两条 404 分类不同，但都不可重试**——安全性质守住了。

use agent_core::ErrorClass;

use crate::wire;

pub(crate) fn classify(status: u16, body: &str) -> ErrorClass {
    wire::errors::classify(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DeepSeek 形状：标准 `error.type`，400。（174 D 组实测原文）
    #[test]
    fn standard_openai_error_shape_is_bad_request() {
        let body = r#"{"error":{"code":"invalid_request_error","message":"The supported API model names are deepseek-v4-pro or deepseek-v4-flash, but you passed definitely-not-a-real-model-xyz.","param":null,"type":"invalid_request_error"}}"#;
        assert_eq!(classify(400, body), ErrorClass::BadRequest);
    }

    /// Kimi 形状：**模型名不存在给 404**（不是 400）。状态码兜底要把它判成
    /// `BadRequest` 而不是当成网络层的找不到——重试它毫无意义。（174 D 组实测原文）
    #[test]
    fn kimi_style_404_for_a_missing_model_is_still_bad_request() {
        let body = r#"{"error":{"message":"Not found the model definitely-not-a-real-model-xyz or Permission denied","type":"resource_not_found_error"}}"#;
        assert_eq!(classify(404, body), ErrorClass::BadRequest);
    }

    /// GLM 形状：`error` 里**没有 `type`**，只有 `code`/`message`，且 code 是
    /// 字符串数字。按 `type` 判会落空，靠状态码兜底。（174 D 组实测原文）
    #[test]
    fn glm_style_error_without_a_type_field_falls_back_to_status() {
        let body = r#"{"error":{"code":"1214","message":"modelCode：不存在"}}"#;
        assert_eq!(classify(400, body), ErrorClass::BadRequest);
    }

    /// 完全不是 OpenAI 形状的错误体（GLM 路径打错时的 Spring 风格 404）→ `Unknown`。
    ///
    /// **`Unknown` 才是对的，不是 `BadRequest`。** 一个通用 adapter 收到裸 404 +
    /// 非 OpenAI 错误体，最可能的真相是**用户的 `base_url` 填错了**（174 里我自己
    /// 就是这么撞出来的：给 GLM 拼了个 `/v1` 结果整组 404）。那是配置错误，
    /// 要人去看，不是「这次请求的内容不合法」。`Unknown` 的语义正是「保守处理、
    /// 不自动重试」——两条 404 分类不同但都不可重试，安全性质守住了。
    #[test]
    fn a_bare_404_with_a_non_openai_body_is_unknown_not_bad_request() {
        let body = r#"{"timestamp":"2026-08-13T04:35:22.960+00:00","status":404,"error":"Not Found","path":"/v4/v1/chat/completions"}"#;
        assert_eq!(classify(404, body), ErrorClass::Unknown);
    }

    /// 鉴权：401 → `Auth`，重试无意义，要人换 key。
    #[test]
    fn unauthorized_is_auth() {
        assert_eq!(
            classify(401, r#"{"error":{"message":"invalid api key"}}"#),
            ErrorClass::Auth
        );
    }

    /// 限流：429 → `Retryable`。
    #[test]
    fn rate_limited_is_retryable() {
        assert_eq!(
            classify(429, r#"{"error":{"type":"rate_limit_error"}}"#),
            ErrorClass::Retryable
        );
    }

    /// 空响应体不该 panic——本地实现（Ollama/vLLM）在崩溃路径上什么都不返回是常事。
    #[test]
    fn an_empty_body_does_not_panic() {
        let _ = classify(500, "");
        let _ = classify(200, "");
    }
}
