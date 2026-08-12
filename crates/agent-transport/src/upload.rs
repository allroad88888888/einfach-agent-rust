//! Moonshot 图片上传请求的 multipart 编码与响应翻译。
//!
//! **issue 113 平台适配**：类型定义（[`ImageUpload`]/[`UploadError`]/
//! [`MAX_IMAGE_BYTES`]）与 multipart 编码/响应解析是平台无关的纯逻辑，两边
//! 共用，就长在这个文件里，不拆。真正碰网络的 [`send`] 是 native 专属（吃
//! `ureq::Agent`），本文件内用 `#[cfg(not(target_arch = "wasm32"))]` 单独
//! 包住；wasm 侧的等价物是 `fetch_upload.rs` 的 `send()`，两者共享的
//! `multipart_body`/`boundary_for`/`escaped_header_value`/`redact`/
//! `UploadResponse` 从 `private` 改成 `pub(crate)`，除此之外**语义一个字
//! 没动**——不是本 issue 该碰的事（M11 的东西）。

#[cfg(not(target_arch = "wasm32"))]
use std::io::Read;

use serde::Deserialize;

/// Moonshot 同时限制单文件和整个请求体为 100 MiB。
///
/// **这是传输层的上限，不是各宿主的上限。** 浏览器宿主另有一个小得多的
/// 2 MiB 闸（`agent_wasm::vision::MAX_BROWSER_IMAGE_BYTES`，M14 issue 119 §五-1）
/// ——那条管的是 IndexedDB 的 origin 配额，一张 100 MiB 的图能把整个配额吃光，
/// 跟「Moonshot 收不收」是两回事。**别把这两个数字合成一个。**
pub const MAX_IMAGE_BYTES: usize = 100 * 1024 * 1024;

/// 一张待上传图片的元数据与原始字节。
#[derive(Clone, Copy, Debug)]
pub struct ImageUpload<'a> {
    /// 用户可见的原始文件名。
    pub file_name: &'a str,
    /// 文件的 MIME 类型，例如 `image/png`。
    pub mime_type: &'a str,
    /// 不经文本编码转换的原始图片字节。
    pub bytes: &'a [u8],
}

/// 图片上传失败的可分类原因。变体和调试输出都不保存 API key。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadError {
    /// 文件或 multipart 请求体会超过官方 100 MiB 限制，尚未发起 HTTP 请求。
    TooLarge {
        actual_bytes: usize,
        limit_bytes: usize,
    },
    /// 服务端以 401 拒绝了认证。
    Unauthorized,
    /// 服务端接受请求但拒绝了它（413 与其他状态码可由 `status` 区分）。
    ProviderRejected { status: u16 },
    /// 建连、TLS 或读写失败，尚未得到可分类的 HTTP 答复。
    Network { message: String },
    /// HTTP 成功响应没有提供可用的文件 id。
    InvalidResponse { message: String },
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::TooLarge {
                actual_bytes,
                limit_bytes,
            } => {
                write!(
                    f,
                    "图片上传超过大小限制：{actual_bytes} bytes（上限 {limit_bytes} bytes）"
                )
            }
            UploadError::Unauthorized => write!(f, "图片上传认证失败（HTTP 401）"),
            UploadError::ProviderRejected { status } => {
                write!(f, "图片上传被服务商拒绝（HTTP {status}）")
            }
            UploadError::Network { message } => write!(f, "图片上传网络失败：{message}"),
            UploadError::InvalidResponse { message } => write!(f, "图片上传响应无效：{message}"),
        }
    }
}

impl std::error::Error for UploadError {}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn send(
    agent: &ureq::Agent,
    base_url: &str,
    api_key: &str,
    image: ImageUpload<'_>,
) -> Result<String, UploadError> {
    if image.bytes.len() > MAX_IMAGE_BYTES {
        return Err(UploadError::TooLarge {
            actual_bytes: image.bytes.len(),
            limit_bytes: MAX_IMAGE_BYTES,
        });
    }

    let boundary = boundary_for(image.bytes);
    let body = multipart_body(&boundary, image);
    if body.len() > MAX_IMAGE_BYTES {
        return Err(UploadError::TooLarge {
            actual_bytes: body.len(),
            limit_bytes: MAX_IMAGE_BYTES,
        });
    }

    let url = format!("{}/files", base_url.trim_end_matches('/'));
    match agent
        .post(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .set("Accept", "application/json")
        .send_bytes(&body)
    {
        Ok(response) => response_reference(response, api_key),
        Err(ureq::Error::Status(401, _)) => Err(UploadError::Unauthorized),
        Err(ureq::Error::Status(status, _)) => Err(UploadError::ProviderRejected { status }),
        Err(ureq::Error::Transport(error)) => Err(UploadError::Network {
            message: redact(&error.to_string(), api_key),
        }),
    }
}

/// Moonshot `/files` 接口的响应形状。native/wasm 两条 `send()` 都要从响应体
/// 解出这个，`pub(crate)` 给 `fetch_upload.rs` 复用。
#[derive(Deserialize)]
pub(crate) struct UploadResponse {
    id: String,
}

#[cfg(not(target_arch = "wasm32"))]
fn response_reference(response: ureq::Response, api_key: &str) -> Result<String, UploadError> {
    let mut body = String::new();
    response
        .into_reader()
        .read_to_string(&mut body)
        .map_err(|error| UploadError::Network {
            message: redact(&error.to_string(), api_key),
        })?;
    let response: UploadResponse =
        serde_json::from_str(&body).map_err(|_| UploadError::InvalidResponse {
            message: "缺少合法 JSON 文件 id".to_string(),
        })?;
    upload_response_reference(response)
}

/// `UploadResponse` → `ms://` 引用的翻译，从「怎么把响应体读成字符串」里
/// 拆出来，是 native（`ureq::Response::into_reader`）和 wasm（`fetch` 的
/// `Response::text()`）两条路径唯一共享的收尾逻辑。
pub(crate) fn upload_response_reference(response: UploadResponse) -> Result<String, UploadError> {
    if response.id.is_empty() {
        return Err(UploadError::InvalidResponse {
            message: "文件 id 为空".to_string(),
        });
    }
    Ok(format!("ms://{}", response.id))
}

pub(crate) fn multipart_body(boundary: &str, image: ImageUpload<'_>) -> Vec<u8> {
    let mut body = Vec::with_capacity(image.bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\nimage\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            escaped_header_value(image.file_name),
            escaped_header_value(image.mime_type)
        )
        .as_bytes(),
    );
    body.extend_from_slice(image.bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// 选一个不会出现在文件字节里的 boundary，避免二进制内容被服务端误判成分隔符。
pub(crate) fn boundary_for(bytes: &[u8]) -> String {
    for suffix in 0u64.. {
        let boundary = format!("----einfach-agent-image-{suffix}");
        let delimiter = format!("--{boundary}");
        if !bytes
            .windows(delimiter.len())
            .any(|window| window == delimiter.as_bytes())
        {
            return boundary;
        }
    }
    unreachable!("u64 boundary 后缀不可能全部出现在一个最大 100 MiB 的文件中")
}

/// multipart 头部不可出现换行；替换可防止用户文件名或 MIME 类型注入额外头部。
fn escaped_header_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ").replace('"', "'")
}

/// 抹掉错误信息里可能带出的 API key——native/wasm 两条 `send()` 的错误分支
/// 都要用。
pub(crate) fn redact(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        message.to_string()
    } else {
        message.replace(api_key, "[REDACTED]")
    }
}
