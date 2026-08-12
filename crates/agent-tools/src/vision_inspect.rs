//! `srv:vision/inspect`（s5）：写死 Kimi 3（`kimi-k3`）的识图工具。
//!
//! 本文件只放**声明与编排**：`inspect()` 依次调用 [`parse_input`]，再委托给
//! 两个兄弟模块——拆分是 issue 126 的产物（红线 9：本文件曾经 487 行，含
//! 三段被同步 IO 包住的纯逻辑）：
//!
//! - `vision_kimi_wire`——Kimi 请求/响应的**线格式**：纯函数（无 IO、无时钟、
//!   无随机），`chat_body` / `parse_content` / `extension_for` 是跨 crate 契约，
//!   `agent-wasm` 也要用（issue 127）。
//! - `vision_source`——**按链接取字节 + 发请求**：本模块独有的 `std::fs` 与
//!   网络 IO，native-only，不进任何跨 crate 契约。
//!
//! # 安全模型：图片字节从不进任何模型上下文
//!
//! 主模型（对话/历史/prompt）永远只看到 `{ "image": "/uploads/<id>" }` 这样的
//! **链接字符串**。工具执行时把字节从本地取回内存（仅在这一次执行里存在）、
//! 经 Kimi files API 上传换成 `ms://` 引用，再带着引用进 chat completions——
//! 识别结果（纯文本）才是回到主 agent 的东西。
//!
//! **这条承诺的边界必须说清楚：它管的是「字节不进模型上下文」（不进会话历史
//! /prompt/journal，模型永远只能通过 `image_url`/`ms://` 引用间接触碰图
//! 片），不是「字节不落盘」。** 三种宿主形态本来就各自把字节落在别处，而且
//! 落的方式相同——只是「落盘」和「不进模型上下文」是两件独立的事，前者是实
//! 现细节，后者才是安全边界：
//!
//! | 形态 | 字节住哪（落盘/落库） |
//! |---|---|
//! | server | 上传目录 `<dir>/<id>`（`agent-server/src/http/uploads.rs`，进程退出即丢） |
//! | CLI | 用户本机文件——`image` 本来就是本机相对路径，字节一直在磁盘上 |
//! | 浏览器 | IndexedDB 的 `images` object store，会话级生命周期（docs/issues/119 §五） |
//!
//! 三种形态都在落盘/落库，这条安全承诺**从未被打破过**——它只承诺链接不落地
//! 为字节进入 prompt，不承诺字节本身住在内存里。
//!
//! （这条边界曾经写得更绝对——把「不进模型上下文」和「字节不落地存在」两件
//! 事混成一句话，不准确且过强：server 形态本来就把字节落盘了，那种写法字面
//! 意思会让下一个人误以为「图片进 IndexedDB」违反了既有承诺。**别把它改回
//! 去**——一个错的理由比没有理由更糟，`docs/issues/099-send-plan.md` §「主
//! 会话复核修正的一处」为同一类问题付过一次学费；订正记录见
//! docs/issues/131-vision-persistence-wording.md。）
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

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::ToolSpec;
use agent_transport::Client;
use serde_json::{Value, json};

use crate::ToolError;
use crate::exec::tool_err;
use crate::vision_source;

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
    let (bytes, mime) = vision_source::resolve_bytes(vision, &image)?;
    let file_ref = vision_source::upload(vision, &mime, &bytes)?;
    vision_source::chat_completion(vision, &file_ref, &question)
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

#[cfg(test)]
#[path = "vision_inspect_tests.rs"]
mod tests;
