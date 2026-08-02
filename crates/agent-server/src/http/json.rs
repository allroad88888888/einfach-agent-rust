//! [`ApiJson`]：`axum::Json<T>` 的入参版本替身——唯一区别是反序列化失败时把
//! axum 内置的 rejection（纯文本 body，绕过应用自己的错误映射）转成这个 crate
//! 统一的 `{"error":{"code","message"}}` 形状（issue 031「错误统一 JSON」；031
//! 独测钉住的分歧 3：裸 `Json<T>` extractor 在 handler 真正被调用之前就短路
//! 返回它自己的 rejection——这个类型顶替它，把同一个坑焊死）。
//!
//! `message` 是固定文案，不拼回 axum rejection 里的任何片段——那些片段可能
//! 包含请求体的原始内容（比如 serde_json 的语法错误信息里常带出错位置附近的
//! 字符），错误响应不该变成一个把它反射回去的信道（隐私一致性）。

use axum::Json;
use axum::extract::{FromRequest, Request};

use crate::http::error::ApiError;

/// 用法跟 `axum::Json<T>` 完全一样——只是把它当 handler 的入参类型，出参
/// （响应体）该用哪个就还用哪个，这个类型不管响应序列化。
pub(in crate::http) struct ApiJson<T>(pub(in crate::http) T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(_rejection) => Err(ApiError::bad_request("请求体不是合法 JSON，或者字段形状跟期望的不符".to_string())),
        }
    }
}
