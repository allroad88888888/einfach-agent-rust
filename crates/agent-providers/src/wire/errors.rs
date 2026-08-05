//! HTTP 状态 + 响应体 → [`ErrorClass`]，三家共用一套判定。
//!
//! **先按 `error.type` 判，落不到再按状态码**（PROVIDERS.md §四）：三家的状态
//! 码分配不一致——Kimi 把模型名不存在给成 404（别处 404 通常意味着不可恢复的
//! 路径问题），GLM/DeepSeek 给 400；Kimi 把过载给 429，DeepSeek 给 503。
//! `type` 是各家自报的语义，比状态码可信，而且实测下来三家的 `type` 取值都能
//! 落进同一套关键词——`auth` / `balance`|`quota` / `rate_limit`|`overload`|
//! `server_error` / `invalid_request`|`not_found`，不需要为哪一家单独开分支。
//!
//! **402（余额耗尽）必须单列**，而且比 `type` 判得更早——DeepSeek 的 402
//! 响应体里 `type` 是个笼统值（真正说明问题的是 `code`），按 type 判会掉进
//! `BadRequest`。混进限流的后果是系统安静地退避到天荒地老，混进 BadRequest
//! 的后果是永远不重试也不告警：两个方向都错得离谱，所以这一位不走通用路径。
//! Kimi/GLM 没有实测到 402（PROVIDERS.md §四标了「—」），但这条规则本身跟哪
//! 家无关，保留对三家统一生效不会有副作用。
//!
//! 响应体有两种壳：`{"error": {...}}`（HTTP 错误）和裸的 `{"type": ...}`
//! （实测 DeepSeek `tool_choice` 那两个 400 就是裸的），两种都认。

use agent_core::ErrorClass;
use serde_json::Value;

pub fn classify(status: u16, body: &str) -> ErrorClass {
    if status == 402 {
        return ErrorClass::Exhausted;
    }
    if let Some(class) = by_body(body) {
        return class;
    }
    by_status(status)
}

fn by_body(body: &str) -> Option<ErrorClass> {
    let root = serde_json::from_str::<Value>(body).ok()?;
    let err = root.get("error").unwrap_or(&root);

    // 余额相关的话术优先：它不可重试且要立刻告警到人。
    let message = err.get("message").and_then(Value::as_str).unwrap_or("");
    if message
        .to_ascii_lowercase()
        .contains("insufficient balance")
    {
        return Some(ErrorClass::Exhausted);
    }

    let t = err
        .get("type")
        .and_then(Value::as_str)?
        .to_ascii_lowercase();
    // 只认得出的才认，认不出返回 None 交给状态码——把没见过的 type 硬塞进
    // 某一类，就是 402 那种错法。
    if t.contains("auth") {
        Some(ErrorClass::Auth)
    } else if t.contains("balance") || t.contains("quota") {
        Some(ErrorClass::Exhausted)
    } else if t.contains("rate_limit") || t.contains("overload") || t.contains("server_error") {
        Some(ErrorClass::Retryable)
    } else if t.contains("invalid_request") || t.contains("not_found") {
        // `not_found`：Kimi 的模型名错误是 `resource_not_found_error`——404 在
        // 别处通常意味着不可恢复的路径问题，但这里语义就是「请求本身不对」。
        Some(ErrorClass::BadRequest)
    } else {
        None
    }
}

fn by_status(status: u16) -> ErrorClass {
    match status {
        401 | 403 => ErrorClass::Auth,
        402 => ErrorClass::Exhausted,
        400 => ErrorClass::BadRequest,
        429 => ErrorClass::Retryable,
        // 整个 5xx 段都是 Retryable（DeepSeek 用 503 报过载）。**没见过的 5xx
        // 码也算**：RFC 9110 §15 规定客户端必须把认不出的状态码当成同段的 x00
        // 处理，所以 599 等价于 500 = 服务端故障 = 可重试。
        500..=599 => ErrorClass::Retryable,
        _ => ErrorClass::Unknown,
    }
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

    /// 402 单列：**不管响应体怎么写**都是 `Exhausted`，绝不混进限流或 BadRequest。
    #[test]
    fn payment_required_is_always_exhausted() {
        let ds = r#"{"error":{"message":"Insufficient Balance","type":"unknown_error","code":"invalid_request_error"}}"#;
        assert_eq!(classify(402, ds), ErrorClass::Exhausted);
        assert_eq!(classify(402, ""), ErrorClass::Exhausted);
        // 状态码没给 402、只在话术里说余额不足，也要判出来。
        assert_eq!(classify(200, ds), ErrorClass::Exhausted);
    }

    #[test]
    fn status_fallback_covers_the_five_buckets() {
        assert_eq!(classify(401, ""), ErrorClass::Auth);
        assert_eq!(classify(400, ""), ErrorClass::BadRequest);
        assert_eq!(classify(429, ""), ErrorClass::Retryable);
        assert_eq!(classify(503, ""), ErrorClass::Retryable);
        assert_eq!(classify(500, "not json at all"), ErrorClass::Retryable);
        // 没被分配过的 5xx 也是 Retryable：RFC 9110 §15，认不出的状态码按同段
        // 的 x00 处理。5xx 段整体可重试，不因为码没见过就改判。
        assert_eq!(
            classify(599, r#"{"error":{"type":"mystery"}}"#),
            ErrorClass::Retryable
        );
        // 5xx 之外认不出的既不重试也不当成功——保守。
        assert_eq!(classify(404, ""), ErrorClass::Unknown);
        assert_eq!(
            classify(418, r#"{"error":{"type":"teapot"}}"#),
            ErrorClass::Unknown
        );
    }

    /// type 比状态码更可信：状态码是 400，但 type 说鉴权失败 → Auth。
    #[test]
    fn type_wins_over_status() {
        let body = r#"{"error":{"type":"invalid_authentication_error"}}"#;
        assert_eq!(classify(400, body), ErrorClass::Auth);
        let body = r#"{"error":{"type":"engine_overloaded_error"}}"#;
        assert_eq!(classify(404, body), ErrorClass::Retryable);
    }

    /// Kimi 实测：模型名错误是 404 + `resource_not_found_error`，但语义是
    /// 「请求本身不对」，不是「路径不可恢复」——按 type 关键词判成 BadRequest，
    /// 不掉进 404 的默认 `Unknown`。
    #[test]
    fn not_found_type_overrides_the_404_default() {
        let body = r#"{"error":{"message":"model not found","type":"resource_not_found_error"}}"#;
        assert_eq!(classify(404, body), ErrorClass::BadRequest);
    }
}
