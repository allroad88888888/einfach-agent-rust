//! `inspectImage` 背后的四步编排（issue 127）：`upload_image_async` →
//! `ms://<id>` → `agent_tools::chat_body` → `post_json_async` →
//! `agent_tools::parse_content`。四步都是现成件（125/126），本模块只接线。
//!
//! # 2 MiB 闸：为什么不是 `agent_transport::MAX_IMAGE_BYTES`
//!
//! [`MAX_BROWSER_IMAGE_BYTES`] 管的是浏览器侧 IndexedDB 存储配额（119 §五-1：
//! 配额是整个 origin 共享的，一张大图能把它吃光）；`agent_transport::
//! MAX_IMAGE_BYTES`（100 MiB）管的是 Moonshot 官方对单文件/请求体的传输上限。
//! 两者是完全不同的约束层，**不是同一个数字精度不同、是两件事**，所以各自
//! 独立成常量，谁都不该借用另一个的值。
//!
//! （`agent_transport::MAX_IMAGE_BYTES` 那边的文档本该反向指回这里一句，但
//! 127 这次改动的范围明确不含 `crates/agent-transport/`——留给下一个碰那个
//! 文件的人补上，不在本条里顺手碰不该碰的 crate。）
//!
//! # Kimi 的 base_url/api_key 从哪来：一个独立的 `vision` 配置段
//!
//! `HostConfig`（114d，`crate::config`）今天只装**一家** provider 的配置，
//! 跟 `AgentHost::send()` 用的是同一份——但识图写死 Kimi 3
//! （`vision_inspect.rs:1`），主对话选的可能是 DeepSeek/GLM，这时候 Kimi
//! 的连接信息从哪来？
//!
//! native 侧已经回答过这个问题：`agent-cli::main::resolve_vision` 与
//! `agent-server::bootstrap::resolve_vision` 都不从「当前默认 provider」派生
//! Kimi 的 key，而是从 `providers.toml` 里一个独立的 `[providers.kimi]` 段
//! 直接查——不管默认 provider 是谁，`root.providers.get("kimi")` 都是同一次
//! 查找，互不影响。
//!
//! 这里照抄同一个判断：页面配置 JSON 顶层加一个独立的 **`vision`** 段
//! （`{"base_url":"…","api_key":"…"}`，`model` 可选，缺省
//! [`DEFAULT_KIMI_MODEL`]），跟 `provider`/`base_url`/`api_key` 那几个主对话
//! 字段完全独立——哪怕主 `provider` 恰好也是 `"kimi"`，也**不**隐式复用主
//! key。不做「provider 是 kimi 就复用主 key」这种条件复用：那是一条会让人
//! 费解的隐式分支（同一次配置,有时候免费有时候要多填一遍），也偏离了 native
//! 那条「总是独立查」的先例——两个目标要是这条判断都不一样，排查会是噩梦
//! （111 决策原话的同一条精神）。
//!
//! 这个段**没有解析入口挂在 `crate::config::HostConfig` 上**——那个类型不在
//! 127 允许改动的文件范围内。[`KimiVisionConfig::parse`] 因此对同一份
//! `config_json` 字符串做第二次、独立的 `serde_json` 解析，只认 `vision`
//! 这一个顶层键；`HostConfig::parse` 那次解析看不认识的多余键会被 serde
//! 默认忽略，两次解析互不干扰。
//!
//! `vision` 段缺失，或 `base_url`/`api_key` 缺失/为空 → [`KimiVisionConfig::
//! parse`] 返回 `None`，**不是构造期硬错误**——跟主 provider 那份
//! `ProviderConfig::resolve_key()` 把空字符串当「没填」同一个宽容度：页面可以
//! 先把 `AgentHost` 建起来，用户之后再补 Kimi key，不影响主对话能不能开始。
//! 真正没配的后果留到 [`inspect`] 被调用时才 reject，措辞含 `not_configured`，
//! 对齐 `agent-tools/src/vision_inspect.rs` 那条同名错误。
//!
//! # key 不打印、不序列化
//!
//! [`KimiVisionConfig`] 不派生 `Debug`（跟 `crate::config::HostConfig` 同一条
//! 理由：派生的 `Debug` 会把字段值原样打出来）。它只活在 `Inner.vision` 里，
//! 被 [`inspect`] 借用后直接交给 `agent_transport::Client` 的两个 async
//! 入口——那两个入口自己的 redact 逻辑保证网络错误消息里不含 key（125）。

use agent_transport::{Client, ImageUpload};

/// 浏览器侧单张图上限，119 §五-1 拍板。**不是**
/// `agent_transport::MAX_IMAGE_BYTES`（Moonshot 100 MiB 传输上限）——两个数字
/// 管的是不同的约束层，各自看模块文档，别互相替换。
pub(crate) const MAX_BROWSER_IMAGE_BYTES: usize = 2 * 1024 * 1024;

/// 识图写死的模型（`vision_inspect.rs:1`）。留成可被 `vision.model` 覆盖的
/// 字段只为测试注入假模型名，跟 native `VisionRuntime.kimi_model` 字段同一个
/// 理由。
const DEFAULT_KIMI_MODEL: &str = "kimi-k3";

/// 一个独立于主对话 provider 的 Kimi 连接配置，见模块文档「key 从哪来」。
pub(crate) struct KimiVisionConfig {
    base_url: String,
    api_key: String,
    model: String,
}

impl KimiVisionConfig {
    /// 从页面给的整份配置 JSON 里只挑 `vision` 这一段解析，宽容处理：段缺失、
    /// 或 `base_url`/`api_key` 缺失/为空，一律 `None`（模块文档「key 从哪来」
    /// 那条宽容度）。JSON 本身解析不了也是 `None`——那种情况
    /// `HostConfig::parse` 已经在构造期给出过明确错误，这里不重复报。
    pub(crate) fn parse(config_json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(config_json).ok()?;
        let vision = value.get("vision")?;
        let base_url = non_empty_str(vision, "base_url")?;
        let api_key = non_empty_str(vision, "api_key")?;
        let model = vision
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_KIMI_MODEL)
            .to_owned();
        Some(KimiVisionConfig {
            base_url,
            api_key,
            model,
        })
    }
}

fn non_empty_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `inspectImage` 背后的四步编排，见模块文档开头。
///
/// 顺序：先判断有没有配置（缺配置直接 `not_configured`），再判断大小（超限
/// 直接 reject，**不建 `Client`、不发任何网络请求**），最后才是上传 + chat
/// 两次真实网络往返。
///
/// # `Err` 的形状是契约：`<码>：<细节>`
///
/// **每一条失败路径都以错误码开头**，码取自 `agent_tools::vision_inspect` 那套
/// （`not_configured` / `too_large` / `bad_input` / `upload_failed` /
/// `provider_error` / `invalid_response`），后面跟一个全角冒号和人读的细节。
///
/// 这条契约是 130 的真机验收逼出来的：`www/vision-tool.js` 要把失败翻译成模型看到
/// 的 `[码] 细节`，而在补这几个码之前它只能**按中文散文前缀认字符串**
/// （「Kimi 图片上传失败：」之类）。那种耦合的症状极其隐蔽——改一句提示语，浏览器
/// 侧就把 `upload_failed` 静默重分类成兜底的 `provider_error`：不报错、不断编译、
/// 没有任何测试盯着，只有排查故障的人会觉得「这错怎么归错类了」。
///
/// **加错误路径时必须带码。** 没有码的那一条会被 JS 归进兜底类别，而兜底类别的存在
/// 是为了应付未知，不是为了替这里偷懒。
pub(crate) async fn inspect(
    vision: Option<&KimiVisionConfig>,
    bytes: Vec<u8>,
    mime: String,
    question: String,
) -> Result<String, String> {
    let vision = vision.ok_or_else(|| {
        "not_configured：识图未配置——页面配置 JSON 缺少 vision.base_url / \
         vision.api_key，没有默认 key（127 硬约束）"
            .to_string()
    })?;
    if bytes.len() > MAX_BROWSER_IMAGE_BYTES {
        return Err(format!(
            "too_large：图片 {actual} bytes 超过浏览器上限 {limit} bytes\
             （MAX_BROWSER_IMAGE_BYTES，不是 Moonshot 那个 100 MiB 传输上限，\
             两者是两回事，见 vision.rs 模块文档）",
            actual = bytes.len(),
            limit = MAX_BROWSER_IMAGE_BYTES,
        ));
    }

    let client = Client::new();
    let file_name = format!("uploaded-image.{}", agent_tools::extension_for(&mime));
    let file_ref = client
        .upload_image_async(
            &vision.base_url,
            &vision.api_key,
            ImageUpload {
                file_name: &file_name,
                mime_type: &mime,
                bytes: &bytes,
            },
        )
        .await
        .map_err(|e| format!("upload_failed：Kimi 图片上传失败：{e}"))?;

    let url = format!("{}/chat/completions", vision.base_url.trim_end_matches('/'));
    let body = agent_tools::chat_body(&vision.model, &file_ref, &question);
    let payload =
        serde_json::to_vec(&body).map_err(|e| format!("bad_input：请求体构造失败：{e}"))?;
    let (_status, text) = client
        .post_json_async(&url, &vision.api_key, &payload)
        .await
        .map_err(|e| format!("provider_error：Kimi 识别请求失败：{e}"))?;

    agent_tools::parse_content(&text).map_err(|e| format!("{}：{}", e.code, e.message))
}
