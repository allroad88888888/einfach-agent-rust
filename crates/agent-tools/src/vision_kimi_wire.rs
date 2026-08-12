//! Kimi chat completions 的线格式（issue 126，从 `vision_inspect.rs` 摘出）。
//!
//! 三个 `pub fn` 都是**纯函数**：无 IO、无时钟、无随机，相同入参永远产出相同
//! 结果。这是跨 crate 契约——`agent-wasm`（issue 127）在浏览器侧要拼同一个
//! Kimi 请求体、解析同一个 Kimi 响应，不该重写一遍会漂的实现（见
//! docs/issues/119 §四那张「JS 与 Rust 分工」表：provider 协议归 Rust）。
//!
//! **只摘，不改**：`chat_body` 产出的 JSON 必须跟摘出前 `chat_completion` 里
//! 那个字面量逐字节相同，包括字段顺序——这条请求体已经用真实 Kimi 账号验证
//! 过（`end_to_end_uploads_bytes_then_chats_with_ms_reference`），任何「顺手
//! 优化」都是在改一个已经真机验过的请求体，见 docs/issues/126。
//!
//! 顶层与嵌套对象的键序不用特意维护：本仓 `serde_json` 不开 `preserve_order`，
//! `Map` 是 BTreeMap 后端，序列化时自动排成字典序，跟 `json!` 字面量里写的
//! 先后顺序无关（红线 11 的「注意」——别为了保证顺序去引入 IndexMap）。真正
//! 要看住的是 `content` **数组**的顺序：数组是 `Vec`，顺序由插入顺序决定，
//! 不受 BTreeMap 排序影响，所以 [`chat_body`] 把它写死。

use serde_json::{Value, json};

use crate::ToolError;
use crate::exec::tool_err;

/// Kimi chat completions 的请求体。`content[0]` 恒为 `image_url`、
/// `content[1]` 恒为 `text`——**顺序是这个函数契约的一部分，不是实现细节**。
/// Kimi 那边对顺序敏不敏感没验证过，我们自己先别漂（issue 126 验收）。
pub fn chat_body(model: &str, file_ref: &str, question: &str) -> Value {
    json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": file_ref } },
                { "type": "text", "text": question }
            ]
        }]
    })
}

/// 解析 Kimi chat completions 响应，取 `choices[0].message.content`。三类
/// 失败都落 `invalid_response`：响应不是合法 JSON、缺 `choices`、`choices`
/// 存在但取不到 `message.content` 字符串（不细分成不同错误码）。
pub fn parse_content(text: &str) -> Result<String, ToolError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| tool_err("invalid_response", format!("Kimi 识别响应不是合法 JSON：{e}")))?;
    let content = value["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| tool_err("invalid_response", "Kimi 识别响应缺少 choices[0].message.content"))?;
    Ok(content.to_owned())
}

/// mime → 上传文件名用的扩展名。已知四种图片 mime 精确匹配，其余一律落
/// `"bin"`（调用方用它拼 `uploaded-image.<ext>`）。
pub fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

#[cfg(test)]
#[path = "vision_kimi_wire_tests.rs"]
mod tests;
