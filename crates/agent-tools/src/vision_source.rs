//! 按链接取字节 + 发请求（issue 126，从 `vision_inspect.rs` 摘出）。
//!
//! 跟 `vision_kimi_wire`（Kimi 的线格式）反过来：这里全是 IO——本地文件系统
//! （`VisionLinkSource::UploadDir` / `LocalRoot`）与走 `agent_transport::Client`
//! 的网络请求。native-only，不进任何跨 crate 契约——浏览器侧走自己的
//! IndexedDB + fetch（见 docs/issues/119 §四），这个模块不需要在 wasm 上跑，
//! 只需要在 wasm 上**编译**（agent-tools 整个 crate 的既有约束）。

use std::path::Path;

use agent_transport::ImageUpload;
use serde_json::Value;

use crate::ToolError;
use crate::exec::{Resolved, resolve_in_root, tool_err};
use crate::vision_inspect::{VisionLinkSource, VisionRuntime};
use crate::vision_kimi_wire::{chat_body, extension_for, parse_content};

/// 按链接取字节：字节只在这一层进内存，随后立即上传换 `ms://` 引用。
pub(crate) fn resolve_bytes(
    vision: &VisionRuntime,
    image: &str,
) -> Result<(Vec<u8>, String), ToolError> {
    match &vision.link_source {
        VisionLinkSource::UploadDir(dir) => {
            let id = image
                .strip_prefix("/uploads/")
                .ok_or_else(|| {
                    tool_err(
                        "bad_input",
                        format!("image 必须是本地上传链接（/uploads/<id>），收到：{image}"),
                    )
                })?
                .to_owned();
            // 上传端点只发这种 id；字符白名单挡住路径穿越。
            if !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(tool_err("bad_input", format!("上传链接 id 非法：{id}")));
            }
            let bytes = read_uploaded(dir, &id)?;
            let mime = read_uploaded_mime(dir, &id)?;
            Ok((bytes, mime))
        }
        VisionLinkSource::LocalRoot(root) => {
            // root 本身先 canonicalize：CLI 的启动目录可能带 symlink 组件
            // （macOS /var → /private/var），不先解析会让「canonical 结果
            // starts_with(root)」误判成越界。
            let canonical_root = root
                .canonicalize()
                .map_err(|e| tool_err("bad_config", format!("root 无法解析：{e}")))?;
            match resolve_in_root(&canonical_root, image) {
                Ok(Resolved::Existing(path)) => {
                    let bytes = std::fs::read(&path)
                        .map_err(|e| tool_err("read_failed", format!("读取图片失败：{e}")))?;
                    Ok((bytes, mime_from_path(&path)))
                }
                Ok(Resolved::Missing) => Err(tool_err(
                    "not_found",
                    format!("图片文件不存在：{image}"),
                )),
                Err(e) => Err(e),
            }
        }
    }
}

/// 上传目录里的字节文件：canonicalize 之后再读，防 symlink 穿透（id 已过
/// 字符白名单，这里是第二道闸）。
fn read_uploaded(dir: &Path, id: &str) -> Result<Vec<u8>, ToolError> {
    let canonical_dir = dir
        .canonicalize()
        .map_err(|e| tool_err("not_configured", format!("上传目录不可用：{e}")))?;
    let target = canonical_dir
        .join(id)
        .canonicalize()
        .map_err(|_| tool_err("not_found", format!("上传的图片不存在：{id}")))?;
    if !target.starts_with(&canonical_dir) {
        return Err(tool_err("bad_input", "上传 id 越界"));
    }
    std::fs::read(&target).map_err(|e| tool_err("read_failed", format!("读取上传图片失败：{e}")))
}

fn read_uploaded_mime(dir: &Path, id: &str) -> Result<String, ToolError> {
    let mime_path = dir.join(format!("{id}.mime"));
    std::fs::read_to_string(&mime_path)
        .map(|s| s.trim().to_owned())
        .or_else(|_| Ok("application/octet-stream".to_string()))
}

fn mime_from_path(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png".to_string(),
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("webp") => "image/webp".to_string(),
        Some("gif") => "image/gif".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// 复用 transport 的 Kimi files 上传：`ms://<file_id>` 引用。扩展名来自
/// `vision_kimi_wire::extension_for`（Kimi 线格式那半）。
pub(crate) fn upload(vision: &VisionRuntime, mime: &str, bytes: &[u8]) -> Result<String, ToolError> {
    let file_name = format!("uploaded-image.{}", extension_for(mime));
    vision
        .client
        .upload_image(
            &vision.kimi_base_url,
            &vision.kimi_api_key,
            ImageUpload {
                file_name: &file_name,
                mime_type: mime,
                bytes,
            },
        )
        .map_err(|e| tool_err("upload_failed", format!("Kimi 图片上传失败：{e}")))
}

/// Kimi chat completions：`image_url` 带 `ms://` 引用 + 问题 → 识别文本。
/// 请求体来自 `vision_kimi_wire::chat_body`，响应解析走
/// `vision_kimi_wire::parse_content`——本函数只管发请求、收响应。
pub(crate) fn chat_completion(
    vision: &VisionRuntime,
    file_ref: &str,
    question: &str,
) -> Result<String, ToolError> {
    let url = format!(
        "{}/chat/completions",
        vision.kimi_base_url.trim_end_matches('/')
    );
    let body: Value = chat_body(vision.kimi_model.as_ref(), file_ref, question);
    let payload = serde_json::to_vec(&body)
        .map_err(|e| tool_err("bad_input", format!("请求体构造失败：{e}")))?;
    let (_status, text) = vision
        .client
        .post_json(&url, &vision.kimi_api_key, &payload)
        .map_err(|e| tool_err("provider_error", format!("Kimi 识别请求失败：{e}")))?;
    parse_content(&text)
}
