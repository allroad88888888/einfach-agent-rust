//! `POST /sessions/:id/input`：一句用户输入。
//!
//! **fire-and-forget**：这个响应只确认「命令送进了 actor 的队列」，不等轮次
//! 跑完——跑完之后发生的一切（增量文本、工具调用、终态）都在 `GET
//! /sessions/:id/events` 上，这是这个传输设计本来的分工（ARCHITECTURE.md
//! §传输：下行 SSE、上行 POST），没有请求-响应关联 id 可以拿来「等这次调用对应
//! 的那条结果」——`Command`/`SessionEvent` 从 030 起就没有这个字段，031 不为了
//! 这一个端点新引入一条关联机制。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use agent_core::UserImage;
use agent_transport::ImageUpload;

use crate::Command;
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::SessionId;

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
    let images = prepare_images(&state, body.images).await?;
    state.dispatch(
        &session_id,
        Command::Input {
            text: body.text,
            images,
        },
    )?;
    Ok(StatusCode::ACCEPTED)
}

/// 不支持视觉的 adapter 绝不会把这个内部标记序列化到模型请求；它只保留 core
/// 所需的图片元数据，让 adapter 生成 `ImagesDropped` 和可见占位文本。
const NONVISUAL_IMAGE_REFERENCE: &str = "unavailable://nonvisual-image";

/// 验证 HTTP 图片，并且仅为实际会消费引用的 provider 上传它们。
///
/// 阻塞的 ureq 上传在 tokio 的阻塞池，所以它不会占住任何 session actor。非视觉
/// provider 则完全没有网络 IO，字节在这里验证后立即释放。
async fn prepare_images(
    state: &AppState,
    images: Vec<InputImage>,
) -> Result<Vec<UserImage>, ApiError> {
    validate_images(&images)?;
    if !state.template().provider.supports_images() {
        return Ok(images.into_iter().map(nonvisual_image).collect());
    }

    upload_images(state, images).await
}

fn nonvisual_image(image: InputImage) -> UserImage {
    UserImage {
        reference: Arc::from(NONVISUAL_IMAGE_REFERENCE),
        mime: Arc::from(image.mime),
        name: image.name.map(Arc::from),
    }
}

/// 把视觉 provider 的 HTTP 附件变成 core 只会原样保存的上传引用。
async fn upload_images(
    state: &AppState,
    images: Vec<InputImage>,
) -> Result<Vec<UserImage>, ApiError> {
    let template = state.template();
    let upload_base_url = template.upload_base_url.clone();
    let api_key = template.api_key.clone();
    let client = Arc::clone(&template.client);
    let mut uploaded = Vec::with_capacity(images.len());

    for image in images {
        let (reference, mime, name) = tokio::task::spawn_blocking({
            let client = Arc::clone(&client);
            let upload_base_url = upload_base_url.clone();
            let api_key = api_key.clone();
            move || {
                let InputImage { name, mime, bytes } = image;
                let reference = client
                    .upload_image(
                        &upload_base_url,
                        &api_key,
                        ImageUpload {
                            file_name: name.as_deref().unwrap_or("image"),
                            mime_type: &mime,
                            bytes: &bytes,
                        },
                    )
                    .map_err(|error| ApiError::bad_request(error.to_string()))?;
                Ok((reference, mime, name))
            }
        })
        .await
        .map_err(|_| ApiError::bad_request("图片上传任务未能完成"))??;
        uploaded.push(UserImage {
            reference: Arc::from(reference),
            mime: Arc::from(mime),
            name: name.map(Arc::from),
        });
    }

    Ok(uploaded)
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
            vec![InputImage {
                name: None,
                mime: "image/png".to_string(),
                bytes: vec![0; MAX_IMAGE_BYTES + 1],
            }],
        )
        .await
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
