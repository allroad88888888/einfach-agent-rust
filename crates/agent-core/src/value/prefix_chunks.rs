//! 「会话创建期定下的那一列带 label 的文本块 ↔ [`AgentValue::Json`] 数组」的
//! **唯一一处**编解码（134）。
//!
//! [`Slot::PrefixChunks`](crate::graph::Slot::PrefixChunks) 装的就是这个形状。core
//! 对这一列块只知道两件事：**它在会话创建期一次写定**、**它按写入顺序排在 system
//! 段前面**。至于这些文本是谁算出来的（某个"开局工具"、某份配置、还是宿主手填），
//! core 不知道也不该知道——红线 12 的精神：core 里不该出现「时机」「skill」
//! 这类词，它看见的就是一列文本块。
//!
//! ## 为什么**不排序**（跟 [`host_skills`](super::host_skills) 刻意相反）
//!
//! `host_skills::to_value` 落值前按 id 排序，理由是**输入顺序不可靠**：那是客户端
//! 一次 HTTP 请求里给的数组，两次连接完全可能不同序，而它会变成 prompt 字节
//! （红线 11）。这里的输入不是同一类东西：
//!
//! - **顺序本身是信息**。这一列块的先后就是它们在 system 段里该出现的先后
//!   （135 的注册顺序）。排一遍等于把调用方定的编排悄悄换掉——前缀内容跟着变，
//!   缓存该断照样断，还多赔上一次语义损失，一分钱好处都没换到。
//! - **红线 11 要的是「确定」，不是「排序」**。排序只是「输入不确定时把它变确定」
//!   的一种手段，不是目的。这里的确定性由写入点保证：一次写定、之后不改
//!   （[`Session::set_prefix_chunks`](crate::Session::set_prefix_chunks) 只在建会话
//!   时调一次），同一份历史重放两次，顺序逐条相同、字节逐字节相同。
//!
//! 跟 [`summaries`](super::summaries) 是同一条理由的两个实例（那边是追加顺序
//! = 压缩次序，也不排序）。**判据是「写入点能不能保证确定」**，不是「容器是不是
//! `Vec`」。
//!
//! ## 为什么落进 `Json` 而不是给 `AgentValue` 加一个变体
//!
//! `atom_value.rs` 的模块注释定死了：变体集合 026 一次封闭，之后只增 `Slot`。
//! 加一个变体等于一次值 schema 迁移，旧日志会静默解错——比多一层 JSON 编解码贵
//! 得多。近几个槽位（`SendPlan` / `Summaries` / 三个 Host*）走的都是这条路。
//!
//! ## 形状是 `[[label, text], …]`，不是对象数组
//!
//! 二元数组，照 [`summaries`](super::summaries) 的模式：两个字段都是位置固定的
//! 字符串，键名进落盘字节纯属浪费。`serde_json::Value` 的数组保序，所以同一列块
//! 两次序列化逐字节相同（红线 11）。

use std::sync::Arc;

use crate::seam::SystemChunk;
use crate::value::atom_value::AgentValue;

/// 一列前缀块 → [`AgentValue::Json`]，**原样顺序**（见模块文档「为什么不排序」）。
pub(crate) fn to_value(chunks: &[SystemChunk]) -> AgentValue {
    let items: Vec<serde_json::Value> = chunks
        .iter()
        .map(|chunk| {
            serde_json::Value::Array(vec![
                serde_json::Value::String(chunk.label.to_string()),
                serde_json::Value::String(chunk.text.to_string()),
            ])
        })
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(items)))
}

/// 从值里读回那一列块（原样顺序）。
///
/// **有一项解不出来就整份读空，不逐项跳过**——这一条跟 `host_skills::from_value`
/// 刻意不同，理由在「这个值是怎么写出来的」：host skill 是一份**清单**，宿主报进来
/// 一堆互相独立的项，少一项是少一行索引，模型激活它会当场收到 `is_error`，跳过是
/// 可见的。这一列块不是清单，是**一次原子写入的一个整体**（一条 entry、一个值），
/// 写它的只有 `to_value` 一处。所以「其中一项形状不对」不是「有一项过时了」，
/// 是**这份值坏了**；跳掉那一项会拼出一份看着挺像样、中间少一段的 system 前缀，
/// 每一轮都发出去，没有任何一处会响。整份读空至少让「前缀是空的」成为一个
/// 调用方看得见的事实。
///
/// 类型不对（别的槽位误读到这里）或压根没写过：同样回到空，不 panic——这个读取点
/// 也服务恢复路径，一份认不出的历史数据不该让整个会话起不来。
pub(crate) fn from_value(value: &AgentValue) -> Vec<SystemChunk> {
    let Some(array) = value.as_json().and_then(|json| json.as_array()) else {
        return Vec::new();
    };
    let mut chunks = Vec::with_capacity(array.len());
    for item in array {
        // 切片模式钉死「恰好两项」：多一项少一项都是这份值坏了，不是可以将就的
        // 版本差异——多出来的那一项若真有含义，将就着读就是把它悄悄丢了。
        let Some([label, text]) = item.as_array().map(Vec::as_slice) else {
            return Vec::new();
        };
        let (Some(label), Some(text)) = (label.as_str(), text.as_str()) else {
            return Vec::new();
        };
        chunks.push(SystemChunk {
            label: Arc::from(label),
            text: Arc::from(text),
        });
    }
    chunks
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn chunk(label: &str, text: &str) -> SystemChunk {
        SystemChunk {
            label: Arc::from(label),
            text: Arc::from(text),
        }
    }

    /// 往返：两个字段一个不少，**顺序原样保留**。
    #[test]
    fn round_trips_and_keeps_the_written_order() {
        let chunks = vec![chunk("zeta", "后写的"), chunk("alpha", "先写的")];
        let read = from_value(&to_value(&chunks));
        assert_eq!(read, chunks, "不排序：写进去什么顺序，读出来就什么顺序");
    }

    /// 空列表也走同一条路径（`Slot::PrefixChunks` 的默认值就是它）——
    /// 「没写过」和「写了零块」在状态上是同一件事，不该有第二种表示。
    #[test]
    fn the_empty_list_is_an_empty_array_not_null() {
        let value = to_value(&[]);
        assert_eq!(value, crate::graph::Slot::PrefixChunks.default_value());
        assert!(from_value(&value).is_empty());
    }

    /// 红线 11：同一列块两次序列化逐字节相同；顺序不同的两列**不等**
    /// ——这里没有集合语义，顺序就是内容的一部分。
    #[test]
    fn serialization_is_byte_stable_and_order_sensitive() {
        let ab = vec![chunk("a", "一"), chunk("b", "二")];
        let ba = vec![chunk("b", "二"), chunk("a", "一")];
        assert_eq!(to_value(&ab), to_value(&ab));
        assert_ne!(to_value(&ab), to_value(&ba));

        let AgentValue::Json(v) = &to_value(&ab) else {
            panic!("落 Json")
        };
        assert_eq!(
            serde_json::to_string(&**v).unwrap(),
            r#"[["a","一"],["b","二"]]"#
        );
    }

    /// 形状不对：整份读空，**不是**跳过坏的那一项拼出半份前缀（见 `from_value`
    /// 的文档：这是一次原子写入的整体，不是一份清单）。
    #[test]
    fn a_malformed_value_reads_as_empty_instead_of_a_partial_prefix() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());

        for bad in [
            json!([["ok", "正文"], "整项不是数组"]),
            json!([["ok", "正文"], ["少了正文"]]),
            json!([["ok", "正文"], ["label", 3]]),
            json!([["ok", "正文"], ["label", "text", "多一项"]]),
        ] {
            assert!(
                from_value(&AgentValue::Json(Arc::new(bad.clone()))).is_empty(),
                "{bad} 该整份读空，而不是留下前面那一块"
            );
        }
    }
}
