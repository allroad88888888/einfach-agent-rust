//! 带遮罩的 [`TransportError`] 构造器：**任何会带上游文本的错误，都从这里造**。
//!
//! # 为什么值得单独一个文件
//!
//! `redact` 本身（`upload.rs`）早就有了，问题从来不是「有没有遮罩函数」，
//! 而是「调用点会不会漏掉它」。125 落地时的第一版就踩在这条缝上：实现是对的
//! （两条错误路径都调了 `redact`），但配套的 native 测试是**自己 `redact` 一遍、
//! 自己构造 `TransportError`、再断言不含 key**——它证明的是「`redact` 这个函数
//! 管用」，不是「调用点真的调了它」。把 `post_json_async` 里那两句 `redact(...)`
//! 删掉，那条测试照样绿。
//!
//! 锁不住回归的锁等于没有锁（`docs/issues/097-subagent-ingredient-audit.md`
//! §变异检验：「锁死测试不会红就是废的」）。
//!
//! 于是把「遮罩 + 装进 `TransportError`」合成一步，**测试钉的就是调用点会用到的
//! 那一个函数**：删掉这里的 `redact`，测试立刻红。
//!
//! # 这不是类型级保证，是收窄
//!
//! 没有任何东西阻止别处直接 `TransportError::Http { body: raw }`——要做到那一步
//! 得给 `TransportError` 的字段套 newtype，代价远大于收益。这里换到的是：
//! **上游文本进错误的路只剩一条有名字的路**，它有文档、有会红的测试，
//! 而绕过它需要一次显式的、看得见的选择。
//!
//! # 编译目标
//!
//! `#[cfg(any(target_arch = "wasm32", test))]`——生产调用点今天只有 wasm 侧的
//! [`crate::fetch_client`]，但测试要在 native 上跑（`cargo test --workspace`
//! 跑不到 wasm32 目标）。这跟 `line_framer`/`stream_drive` 挂同一套条件，
//! 同一个理由，见 `lib.rs` 那两行的注释。

use crate::TransportError;
use crate::upload::redact;

/// 连接期失败。`message` 是**上游/浏览器给的原文**，可能带 `Authorization`
/// 头的回显——所以它进 `TransportError` 之前必须过 `redact`。
pub(crate) fn connect(attempts: u32, message: &str, api_key: &str) -> TransportError {
    TransportError::Connect {
        attempts,
        message: redact(message, api_key),
    }
}

/// 非 2xx 响应。`body` 是**上游返回的响应体**，有的家会把请求头原样回显
/// （401 尤其常见）——同上，必须过 `redact`。
///
/// **只用于错误路径。** 2xx 的成功响应体不该走这里：`redact` 是字面替换，
/// 用在正常业务正文上有误伤风险，而正常正文本来也不该含 key。
pub(crate) fn http(status: u16, body: &str, api_key: &str) -> TransportError {
    TransportError::Http {
        status,
        body: redact(body, api_key),
    }
}
