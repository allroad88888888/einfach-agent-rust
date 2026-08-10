//! 极简上传端点（s5）：`POST /uploads`（multipart，字段 `file`）→ 把原始字节
//! 写进部署配置的临时上传目录 → 返回 `{"url": "/uploads/<id>"}`；`GET
//! /uploads/{id}` 回原始字节（Content-Type 按上传时记录的 mime）。
//!
//! # 安全边界
//!
//! - 只收白名单图片类型（png/jpeg/webp/gif）与单张 ≤100 MiB（transport 侧
//!   Moonshot 的同一上限 [`agent_transport::MAX_IMAGE_BYTES`]）。
//! - id 只含 `[A-Za-z0-9_-]`（进程 pid + 单调计数器）；取用时 canonicalize
//!   之后再校验落在上传目录内（第二道闸，防 symlink 穿透）。
//! - 图片字节只落这个临时目录（进程退出即丢，由 OS 回收）与 Kimi files API——
//!   **从不进任何模型上下文**：主模型只看到链接字符串，`srv:vision/inspect`
//!   的安全模型见 `agent_tools::vision_inspect` 模块文档。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::http::error::ApiError;
use crate::http::state::AppState;

/// 允许的图片类型白名单（Kimi files API 的 `image` 用途支持这些）。
const ALLOWED_MIME: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

/// 按 mime 校验文件头魔数，拒绝「声明类型与实际内容不符」的假图片。
///
/// 光信客户端 Content-Type 是不够的（curl 随便发 `image/png` 头就能绕过），
/// 魔数校验是第二道闸。四类各自的签名：PNG `\x89PNG\r\n\x1a\n`、JPEG
/// `\xFF\xD8\xFF`、WebP `RIFF....WEBP`（RIFF 容器，偏移 8 处是 WEBP）、GIF
/// `GIF8`（GIF87a/89a 共用前缀）。
fn verify_magic(mime: &str, bytes: &[u8]) -> Result<(), ApiError> {
    let matches = match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xFF\xD8\xFF"),
        "image/webp" => {
            bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
        }
        "image/gif" => bytes.starts_with(b"GIF8"),
        // handler 已按 ALLOWED_MIME 过滤，理论上到不了这里；保守起见仍拒绝。
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "图片内容与声明类型不符（{mime}）：文件头魔数校验失败"
        )))
    }
}

/// 单张图片上限 = transport 侧 Moonshot 的 100 MiB 限制。
const MAX_UPLOAD_BYTES: usize = agent_transport::MAX_IMAGE_BYTES;

/// 进程内临时上传存储：`<dir>/<id>` 是原始字节，`<dir>/<id>.mime` 是 mime。
/// 目录由部署配置（`SessionTemplate::upload_dir`）指定，`save` 时现建——构造
/// 不要求目录已存在，跟 `tools_root` 同一个「调用方给的根目录不保证已存在」
/// 取舍。
pub(crate) struct UploadStore {
    dir: PathBuf,
    next_id: AtomicU64,
}

impl UploadStore {
    pub(crate) fn new(dir: PathBuf) -> Self {
        UploadStore {
            dir,
            next_id: AtomicU64::new(0),
        }
    }

    /// 存一张已校验的图片，返回 `/uploads/<id>` 里的 `<id>`。
    fn save(&self, mime: &str, bytes: &[u8]) -> Result<String, ApiError> {
        verify_magic(mime, bytes)?;
        // 目录不可建、字节写不进去都是**服务端环境问题**，不是调用方过错——5xx。
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            ApiError::internal_error(format!(
                "上传目录不可写：{}（{e}）",
                self.dir.display()
            ))
        })?;
        let id = format!("up-{}-{}", std::process::id(), self.next_id.fetch_add(1, Ordering::Relaxed));
        std::fs::write(self.dir.join(&id), bytes).map_err(|e| {
            ApiError::internal_error(format!("写入上传图片失败：{e}"))
        })?;
        std::fs::write(self.dir.join(format!("{id}.mime")), mime).map_err(|e| {
            ApiError::internal_error(format!("写入上传 mime 失败：{e}"))
        })?;
        Ok(id)
    }

    /// 按 id 取回 (原始字节, mime)。id 先过字符白名单再 canonicalize，双闸防
    /// 路径穿越与 symlink 穿透。
    fn load(&self, id: &str) -> Result<(Vec<u8>, String), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::not_found(format!("上传的图片不存在：{id}")));
        }
        let canonical_dir = self.dir.canonicalize().map_err(|e| {
            ApiError::not_found(format!("上传目录不可用：{e}"))
        })?;
        let target = canonical_dir.join(id).canonicalize().map_err(|_| {
            ApiError::not_found(format!("上传的图片不存在：{id}"))
        })?;
        if !target.starts_with(&canonical_dir) {
            return Err(ApiError::not_found(format!("上传 id 越界：{id}")));
        }
        let bytes = std::fs::read(&target)
            .map_err(|e| ApiError::not_found(format!("读取上传图片失败：{e}")))?;
        let mime = std::fs::read_to_string(self.dir.join(format!("{id}.mime")))
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|_| "application/octet-stream".to_string());
        Ok((bytes, mime))
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `POST /uploads`：multipart 的 `file` 字段 → 存盘 → `{"url": "/uploads/<id>"}`。
pub(crate) async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let store = state
        .uploads()
        .ok_or_else(|| ApiError::not_found("上传端点未启用（未配置 upload_dir）"))?;

    let mut mime: Option<String> = None;
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        ApiError::bad_request(format!("multipart 解析失败：{e}"))
    })? {
        if field.name() == Some("file") {
            mime = field.content_type().map(str::to_owned);
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::bad_request(format!("读取上传内容失败：{e}")))?;
            bytes = Some(data.to_vec());
            break;
        }
    }

    let Some(mime) = mime else {
        return Err(ApiError::bad_request(
            "缺少 file 字段（multipart 表单字段名必须是 file）",
        ));
    };
    if !ALLOWED_MIME.contains(&mime.as_str()) {
        return Err(ApiError::bad_request(format!(
            "不支持的图片类型：{mime}（允许：png / jpeg / webp / gif）"
        )));
    }
    let bytes = bytes.ok_or_else(|| ApiError::bad_request("缺少 file 内容"))?;
    if bytes.is_empty() {
        return Err(ApiError::bad_request("图片内容为空"));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::bad_request(format!(
            "图片超过大小上限（{} bytes）",
            MAX_UPLOAD_BYTES
        )));
    }

    let id = store.save(&mime, &bytes)?;
    Ok(Json(json!({ "url": format!("/uploads/{id}") })).into_response())
}

/// `GET /uploads/{id}`：回原始字节 + 上传时记录的 Content-Type。
pub(crate) async fn get(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, ApiError> {
    let store = state
        .uploads()
        .ok_or_else(|| ApiError::not_found("上传端点未启用（未配置 upload_dir）"))?;
    let (bytes, mime) = store.load(&id)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime)],
        bytes,
    )
        .into_response())
}
