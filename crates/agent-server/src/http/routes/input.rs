//! `POST /sessions/:id/input`：一句用户输入。
//!
//! **fire-and-forget**：这个响应只确认「命令送进了 actor 的队列」，不等轮次
//! 跑完——跑完之后发生的一切（增量文本、工具调用、终态）都在 `GET
//! /sessions/:id/events` 上，这是这个传输设计本来的分工（ARCHITECTURE.md
//! §传输：下行 SSE、上行 POST），没有请求-响应关联 id 可以拿来「等这次调用对应
//! 的那条结果」——`Command`/`SessionEvent` 从 030 起就没有这个字段，031 不为了
//! 这一个端点新引入一条关联机制。

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::Command;
use crate::attachments::{ImageHandle, ImageRegistration};
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::SessionId;
use agent_core::UserImage;

use super::input_limits;

#[derive(Deserialize)]
pub(in crate::http) struct InputRequest {
    text: String,
    #[serde(default)]
    images: Vec<InputImage>,
}

/// 一张还在 HTTP 边界上的图片。验证完成后字节不会进 command、actor 或 store。
#[derive(Deserialize)]
struct InputImage {
    #[serde(default)]
    name: Option<String>,
    mime: String,
    bytes: Vec<u8>,
}

pub(in crate::http) async fn input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<InputRequest>,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId::from(id);
    // 在任何外部 IO 前先确认会话活着，避免给不存在的会话上传付费文件。
    state.session_handle(&session_id)?;
    let images = prepare_images(&state, &session_id, body.images)?;
    if let Err(error) = state.dispatch(
        &session_id,
        Command::Input {
            text: body.text,
            images: images.images,
        },
    ) {
        discard_images(&state, &session_id, &images.handles);
        return Err(error);
    }
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug)]
struct RegisteredImages {
    images: Vec<UserImage>,
    handles: Vec<ImageHandle>,
}

/// 验证 HTTP 图片，并注册到 session-scoped vault。actor/core 只会看到稳定的
/// `attachment://img_*` 引用；无论当前 provider 是否支持视觉，都不会在入口上传。
fn prepare_images(
    state: &AppState,
    owner: &SessionId,
    images: Vec<InputImage>,
) -> Result<RegisteredImages, ApiError> {
    validate_images(&images)?;
    state.attachments().sweep(Instant::now());
    let mut registered = RegisteredImages {
        images: Vec::with_capacity(images.len()),
        handles: Vec::with_capacity(images.len()),
    };

    for image in images {
        let handle = match state.attachments().register(
            owner,
            ImageRegistration {
                mime: &image.mime,
                name: image.name.as_deref(),
                bytes: &image.bytes,
            },
            Instant::now(),
        ) {
            Ok(handle) => handle,
            Err(error) => {
                discard_images(state, owner, &registered.handles);
                return Err(ApiError::bad_request(error.to_string()));
            }
        };
        registered.images.push(UserImage {
            reference: Arc::from(format!("attachment://{}", handle.as_str())),
            mime: Arc::from(image.mime),
            name: image.name.map(Arc::from),
        });
        registered.handles.push(handle);
    }

    Ok(registered)
}

/// 已注册但未进入 actor 的图片必须立即不可读，避免失败请求留下可读取字节。
fn discard_images(state: &AppState, owner: &SessionId, handles: &[ImageHandle]) {
    for handle in handles {
        let _ = state.attachments().evict(owner, handle);
    }
}

fn validate_images(images: &[InputImage]) -> Result<(), ApiError> {
    input_limits::validate_image_quota(images.iter().map(|image| image.bytes.len()))?;
    if images.iter().any(|image| !image.mime.starts_with("image/")) {
        return Err(ApiError::bad_request("附件必须是图片"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use agent_core::SystemChunk;
    use agent_providers::deepseek::DeepSeek;
    use agent_transport::{Client, MAX_IMAGE_BYTES};

    use crate::http::config::{ServerConfig, SessionTemplate};
    use crate::http::state::AppState;
    use crate::registry::ToolTableSpec;

    #[tokio::test]
    async fn oversized_attachment_is_a_distinct_redacted_bad_request_before_network_io() {
        let state = AppState::new(ServerConfig::new(SessionTemplate {
            provider: Arc::new(DeepSeek),
            upload_base_url: "http://127.0.0.1:1".to_string(),
            endpoint: "http://127.0.0.1:1/not-contacted".to_string(),
            api_key: "secret-upload-key".to_string(),
            model: Arc::from("test"),
            tools: ToolTableSpec::Builtin,
            tools_root: std::env::temp_dir().join("agent-server-input-oversized-test"),
            system: vec![SystemChunk {
                label: Arc::from("base"),
                text: Arc::from("test"),
            }],
            client: Arc::new(Client::new()),
            history_cap: None,
            snapshot_every: None,
            provider_timeout: None,
            remote_tool_timeout: None,
            default_sessions_dir: None,
        }));
        let error = prepare_images(
            &state,
            &SessionId::from("oversized"),
            vec![InputImage {
                name: None,
                mime: "image/png".to_string(),
                bytes: vec![0; MAX_IMAGE_BYTES + 1],
            }],
        )
        .expect_err("超过上限的附件必须在 HTTP 边界失败");
        let rendered = format!("{error:?}");

        assert!(
            rendered.contains("超过大小限制"),
            "超大附件必须与 401/5xx 分开：{rendered}"
        );
        assert!(
            !rendered.contains("secret-upload-key"),
            "超大附件错误不得泄露 api key：{rendered}"
        );
    }
}
