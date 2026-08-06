//! 三家共享的 OpenAI 兼容 wire 骨架（issue 023）。
//!
//! 这里只装「三家都一样的机制」：怎么把 core 的消息/工具序列化成 OpenAI 兼容
//! 的 JSON、怎么转义工具名、怎么判前缀漂移与预测命中、怎么把响应体解码成中立
//! 结构、怎么把 HTTP 状态 + 响应体分类成 `ErrorClass`。**块粒度、工具数上限、
//! usage 的 cached 字段路径这些随家而变的数字不在这里**——它们是数据，各家在
//! 自己的 `mod.rs` 里定义常量，调用这里的函数时当参数传进来。这是 025 定下的
//! 原则的延伸：动作相同、只差参数就共享一份实现，不要为每家复制一份
//! （`StreamAccumulator` 就是这条原则的先例）。
//!
//! 段序 `[Tools][System][History]` 是三家实测的共同渲染顺序（PROVIDERS.md
//! §一：改顶层 tools 后冷轮命中全部为 0，连做真前缀匹配的那家也是 0），这条
//! 不因家而变，焊进 `prefix::image`，不做成参数。

pub mod decode;
pub mod errors;
mod image_placeholder;
pub mod messages;
pub mod names;
pub mod numeric;
pub mod prefix;
pub mod tools;

#[cfg(test)]
mod image_encoding_tests;

use serde_json::Value;

/// 规范序列化：`serde_json::Value` 的 `Map` 后端是 `BTreeMap`，输出与插入顺序
/// 无关（红线 11，根 `Cargo.toml` 显式不开 `preserve_order`）。三家的 `encode`
/// 都靠这一点保证同一份料两次组装逐字节相同。
pub fn canonical(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("Value 序列化不会失败")
}
