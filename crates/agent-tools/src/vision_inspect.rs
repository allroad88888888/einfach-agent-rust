//! `srv:vision/inspect`（s5）：写死 Kimi 3（`kimi-k3`）的识图工具。
//!
//! # 安全模型：图片字节不进任何模型上下文
//!
//! 主模型（对话/历史/prompt）永远只看到 `{ "image": "/uploads/<id>" }` 这样的
//! **链接字符串**。工具执行时把字节从本地取回内存（仅在这一次执行里存在）、
//! 经 Kimi files API 上传换成 `ms://` 引用，再带着引用进 chat completions——
//! 识别结果（纯文本）才是回到主 agent 的东西。字节不落消息历史、不进 prompt、
//! 不进任何持久化。
//!
//! # 链接来源（[`VisionLinkSource`]）只有本地两种
//!
//! 用户明确的边界：**仅本地图片，不走公网 URL**。所以 `image` 参数只接受两种
//! 形状，公网 `https://` 一律拒绝：
//!
//! - `UploadDir(dir)`：server 形态，链接形如 `/uploads/<id>`，字节在
//!   `<dir>/<id>`（mime 在 `<dir>/<id>.mime` sidecar）。
//! - `LocalRoot(root)`：CLI 形态，`image` 是 root 内的相对文件路径（本地图片，
//!   路径监狱同文件工具那套 canonicalize 检查）。
//!
//! 工具未配置（`ToolExecutor` 没有 `VisionRuntime`）时报 `not_configured`，不
//! panic。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::ToolSpec;
use agent_transport::{Client, ImageUpload};
use serde_json::{Value, json};

use crate::ToolError;
use crate::exec::{Resolved, tool_err};

/// 工具全名。`srv:` 前缀经名字规则落 `Location::Server`；可逆性不在已知 pure
/// 名单里，保守落 `Irreversible`（调第三方 API 计费，undo 不该重放）。
pub const VISION_INSPECT_TOOL: &str = "srv:vision/inspect";

/// `srv:vision/inspect` 的运行时配置：Kimi 连接 + 链接→字节的来源。
///
/// 全部字段是纯数据（可 `Clone`、可进 `OpenSpec`/`SessionTemplate`），不持有
/// 任何会话状态——链接→字节的解析在 [`VisionLinkSource`] 里按来源本地完成，
/// 不需要闭包或回调。
///
/// **手写 `Debug`，不打印 API key**——只报长度（跟 `agent_transport::config`
/// 的 `ProviderConfig` 同一个硬规矩：key 任何时候不打印）。
#[derive(Clone)]
pub struct VisionRuntime {
    /// Kimi files/chat 共用的 transport client（复用宿主那份 `Arc<Client>`）。
    pub client: Arc<Client>,
    /// Kimi API 基址（例如 `https://api.moonshot.cn/v1`）；上传在尾部追加
    /// `/files`，chat 在尾部追加 `/chat/completions`。
    pub kimi_base_url: String,
    /// Kimi API key。只在这个 struct 里短暂存在（server→runtime 链路），
    /// 绝不进 `ToolSpec`/消息历史/任何持久化。
    pub kimi_api_key: String,
    /// 写死 Kimi 3（`kimi-k3`）。留成字段是为了测试注入假模型名。
    pub kimi_model: Arc<str>,
    /// 链接→字节的来源（仅本地两种，见模块文档）。
    pub link_source: VisionLinkSource,
}

impl VisionRuntime {
    pub fn new(
        client: Arc<Client>,
        kimi_base_url: impl Into<String>,
        kimi_api_key: impl Into<String>,
        kimi_model: impl Into<Arc<str>>,
        link_source: VisionLinkSource,
    ) -> Self {
        VisionRuntime {
            client,
            kimi_base_url: kimi_base_url.into(),
            kimi_api_key: kimi_api_key.into(),
            kimi_model: kimi_model.into(),
            link_source,
        }
    }
}

impl std::fmt::Debug for VisionRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisionRuntime")
            .field("kimi_base_url", &self.kimi_base_url)
            .field("kimi_api_key_len", &self.kimi_api_key.len())
            .field("kimi_model", &self.kimi_model)
            .field("link_source", &self.link_source)
            .finish_non_exhaustive()
    }
}

/// 链接→字节的来源。**只有本地两种**（用户边界：仅本地图片，不走公网 URL）。
#[derive(Clone, Debug)]
pub enum VisionLinkSource {
    /// server：`image` 必须是 `/uploads/<id>`，字节在 `<dir>/<id>`，mime 在
    /// `<dir>/<id>.mime`。
    UploadDir(PathBuf),
    /// CLI：`image` 是 root 内的相对文件路径（路径监狱同 `fs/read` 那套）。
    LocalRoot(PathBuf),
}

/// `srv:vision/inspect` 的声明（模型看到的 name/description/schema）。
pub fn vision_inspect_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(VISION_INSPECT_TOOL),
        description: Arc::from(
            "识别一张本地图片的内容，返回文字描述。image：本地图片链接地址，\
             必填——只接受本地上传返回的链接（形如 /uploads/<id>）或本机相对\
             路径，不接受公网 URL。question：想问这张图的问题，可选，缺省为\
             “这张图片里有什么？”。图片字节只发给识图服务（Kimi 3），\
             不会进入对话历史或模型上下文，你只会拿到识别结果的文本。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "本地图片链接（/uploads/<id> 或相对路径），必填。"
                },
                "question": {
                    "type": "string",
                    "description": "识别问题，可选，缺省为“这张图片里有什么？”。"
                }
            },
            "required": ["image"],
            "additionalProperties": false
        })),
    }
}

/// 执行入口。`vision: None`（工具未配置）→ `not_configured`；其余失败按阶段
/// 分类成 `bad_input` / `not_found` / `upload_failed` / `provider_error` /
/// `invalid_response`。
pub(crate) fn inspect(
    vision: Option<&VisionRuntime>,
    input: &Value,
) -> Result<String, ToolError> {
    let Some(vision) = vision else {
        return Err(tool_err(
            "not_configured",
            "srv:vision/inspect 未配置：需要 providers.toml 的 [providers.kimi] 段，\
             以及上传目录（server）或本地 root（CLI）",
        ));
    };
    let (image, question) = parse_input(input)?;
    let (bytes, mime) = resolve_bytes(vision, &image)?;
    let file_ref = upload(vision, &mime, &bytes)?;
    chat_completion(vision, &file_ref, &question)
}

fn parse_input(input: &Value) -> Result<(String, String), ToolError> {
    let image = input
        .get("image")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| tool_err("bad_input", "srv:vision/inspect 缺少必填参数 image（本地图片链接）"))?;
    if image.is_empty() {
        return Err(tool_err("bad_input", "image 不能为空"));
    }
    let question = input
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("这张图片里有什么？")
        .to_owned();
    Ok((image, question))
}

/// 按链接取字节：字节只在这一层进内存，随后立即上传换 `ms://` 引用。
fn resolve_bytes(vision: &VisionRuntime, image: &str) -> Result<(Vec<u8>, String), ToolError> {
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
            let canonical_root = root.canonicalize().map_err(|e| {
                tool_err("bad_config", format!("root 无法解析：{e}"))
            })?;
            match crate::exec::resolve_in_root(&canonical_root, image) {
                Ok(Resolved::Existing(path)) => {
                    let bytes = std::fs::read(&path).map_err(|e| {
                        tool_err("read_failed", format!("读取图片失败：{e}"))
                    })?;
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
    let canonical_dir = dir.canonicalize().map_err(|e| {
        tool_err("not_configured", format!("上传目录不可用：{e}"))
    })?;
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

/// 复用 transport 的 Kimi files 上传：`ms://<file_id>` 引用。
fn upload(vision: &VisionRuntime, mime: &str, bytes: &[u8]) -> Result<String, ToolError> {
    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    };
    let file_name = format!("uploaded-image.{extension}");
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
fn chat_completion(vision: &VisionRuntime, file_ref: &str, question: &str) -> Result<String, ToolError> {
    let url = format!("{}/chat/completions", vision.kimi_base_url.trim_end_matches('/'));
    let body = json!({
        "model": vision.kimi_model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": file_ref } },
                { "type": "text", "text": question }
            ]
        }]
    });
    let payload = serde_json::to_vec(&body)
        .map_err(|e| tool_err("bad_input", format!("请求体构造失败：{e}")))?;
    let (_status, text) = vision
        .client
        .post_json(&url, &vision.kimi_api_key, &payload)
        .map_err(|e| tool_err("provider_error", format!("Kimi 识别请求失败：{e}")))?;
    parse_content(&text)
}

fn parse_content(text: &str) -> Result<String, ToolError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| tool_err("invalid_response", format!("Kimi 识别响应不是合法 JSON：{e}")))?;
    let content = value["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| tool_err("invalid_response", "Kimi 识别响应缺少 choices[0].message.content"))?;
    Ok(content.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    const KIMI_VISION_MODEL: &str = "kimi-k3";

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-tools-vision-{name}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn runtime(link_source: VisionLinkSource) -> VisionRuntime {
        VisionRuntime::new(
            Arc::new(Client::new()),
            "https://api.moonshot.cn/v1",
            "sk-vision-test-key",
            KIMI_VISION_MODEL,
            link_source,
        )
    }

    #[test]
    fn spec_declares_image_and_question() {
        let spec = vision_inspect_spec();
        assert_eq!(&*spec.name, VISION_INSPECT_TOOL);
        let schema = &spec.schema;
        assert_eq!(schema["required"], json!(["image"]));
        assert_eq!(schema["properties"]["image"]["type"], json!("string"));
        assert_eq!(schema["properties"]["question"]["type"], json!("string"));
        assert!(schema["properties"]["question"]["description"].is_string());
    }

    #[test]
    fn missing_image_is_bad_input() {
        let vision = runtime(VisionLinkSource::LocalRoot(temp_dir("missing")));
        let err = inspect(Some(&vision), &json!({ "question": "有什么？" })).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn not_configured_without_runtime() {
        let err = inspect(None, &json!({ "image": "/uploads/up-1" })).unwrap_err();
        assert_eq!(&*err.code, "not_configured");
    }

    #[test]
    fn public_url_is_rejected_in_upload_dir_mode() {
        let vision = runtime(VisionLinkSource::UploadDir(temp_dir("public-url")));
        let err = inspect(
            Some(&vision),
            &json!({ "image": "https://example.com/cat.png" }),
        )
        .unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn missing_uploaded_file_is_not_found() {
        let dir = temp_dir("missing-upload");
        let vision = runtime(VisionLinkSource::UploadDir(dir));
        let err = inspect(Some(&vision), &json!({ "image": "/uploads/up-999" })).unwrap_err();
        assert_eq!(&*err.code, "not_found");
    }

    #[test]
    fn local_root_resolves_relative_path_within_root() {
        let root = temp_dir("local-root");
        std::fs::write(root.join("cat.png"), b"\x89PNG-fake-bytes").unwrap();
        let vision = runtime(VisionLinkSource::LocalRoot(root));
        // 本地 root 形态：先走到 fake Kimi（这里只验证解析阶段成功取到字节并
        // 开始上传——用不可能连通的 base_url 验证错误发生在 upload 阶段而不是
        // 解析阶段）。
        let mut vision = vision;
        vision.kimi_base_url = format!("http://127.0.0.1:1/v1");
        let err = inspect(Some(&vision), &json!({ "image": "cat.png" })).unwrap_err();
        assert_eq!(
            &*err.code,
            "upload_failed",
            "字节解析应成功、失败应发生在上传阶段：{}",
            err.message
        );
    }

    #[test]
    fn local_root_rejects_dotdot_escape() {
        let root = temp_dir("local-root-escape");
        let vision = runtime(VisionLinkSource::LocalRoot(root));
        let err = inspect(Some(&vision), &json!({ "image": "../secret.png" })).unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }

    // ── 端到端：假 Kimi 上游（files 上传 → chat completions）────────────────

    fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut headers = Vec::new();
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
            headers.push(line.trim_end().to_owned());
        }
        let mut body = vec![0; content_length.unwrap_or(0)];
        reader.read_exact(&mut body).unwrap();
        (request_line.trim_end().to_owned(), body)
    }

    fn write_response(stream: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn end_to_end_uploads_bytes_then_chats_with_ms_reference() {
        let dir = temp_dir("e2e");
        std::fs::write(dir.join("up-7"), b"\x89PNG-raw-pixels").unwrap();
        std::fs::write(dir.join("up-7.mime"), "image/png").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            // 1) files 上传
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            let (request_line, body) = read_request(&mut stream);
            assert_eq!(request_line, "POST /v1/files HTTP/1.1");
            let text = String::from_utf8_lossy(&body);
            assert!(text.contains("name=\"purpose\""), "multipart 缺 purpose 字段");
            assert!(text.contains("name=\"file\""), "multipart 缺 file 字段");
            assert!(text.contains("image/png"), "multipart 缺 mime");
            assert!(body.windows(12).any(|w| w == b"\x89PNG-raw-pix"), "multipart 必须带原始字节");
            write_response(&mut stream, r#"{"id":"file-e2e-1"}"#);

            // 2) chat completions
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            let (request_line, body) = read_request(&mut stream);
            assert_eq!(request_line, "POST /v1/chat/completions HTTP/1.1");
            let text: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(text["model"], json!("kimi-k3"));
            let content = &text["messages"][0]["content"];
            assert_eq!(content[0]["type"], json!("image_url"));
            assert_eq!(content[0]["image_url"]["url"], json!("ms://file-e2e-1"));
            assert_eq!(content[1]["type"], json!("text"));
            assert_eq!(content[1]["text"], json!("这是什么动物？"));
            write_response(
                &mut stream,
                r#"{"choices":[{"message":{"content":"一只橘猫"}}]}"#,
            );
        });

        let mut vision = runtime(VisionLinkSource::UploadDir(dir));
        vision.kimi_base_url = format!("http://127.0.0.1:{port}/v1");
        let result = inspect(
            Some(&vision),
            &json!({ "image": "/uploads/up-7", "question": "这是什么动物？" }),
        )
        .unwrap();

        server.join().unwrap();
        assert_eq!(result, "一只橘猫");
    }
}
