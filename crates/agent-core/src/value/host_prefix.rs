//! 「宿主经 `capabilities.prefix` 声明的开局块 ↔ [`AgentValue::Json`] 数组」的唯一
//! 一处编解码（154，决策 31 的状态位）。
//!
//! `Slot::HostPrefix` 装的就是这个形状：一列 `(name, text)` 对。跟隔壁
//! [`host_tools`](super::host_tools) 是同一类东西——**声明**在 store、**内容怎么来**
//! 由宿主在装配期负责（155/156 的事，本文件不知道也不该知道，红线 12 的精神）。
//!
//! ## 排序：跟 [`host_tools`](super::host_tools) 一样排，跟
//! [`prefix_chunks`](super::prefix_chunks) 刻意不同
//!
//! [`prefix_chunks`] 不排序，因为它的输入顺序**是**信息、且由「一次写定」的写入点
//! 保证确定。这里的输入不是那种东西：它来自宿主一次 HTTP 请求里的
//! `capabilities.prefix` 数组，跟 073 的工具声明同一个不可靠来源——两次连接完全
//! 可能给出不同序，而它会变成 prompt 字节（红线 11）。所以 [`to_value`] 照
//! `host_tools::to_value` 的先例**按名字排序**再落值。
//!
//! ## 反序列化：跟 [`prefix_chunks`](super::prefix_chunks) 一样 all-or-empty，
//! 跟 [`host_tools`](super::host_tools) 刻意不同
//!
//! `host_tools::from_value` 遇到形状不对的单项会跳过它、留下其余——那是一份**清单**，
//! 少一项是少一行索引，模型调不到它会当场报错，跳过是可见的。这里的值不是清单，
//! 是**一次原子写入的一个整体**（一条 entry、一个值，写它的只有 [`to_value`]
//! 一处）：其中一项形状不对不是「有一项过时了」，是这份值坏了，跳掉那一项会拼出
//! 一份看着挺像样、中间少一段的开局块，没有任何一处会响。整份读空至少让
//! 「开局块是空的」成为一个调用方看得见的事实——照
//! [`prefix_chunks::from_value`](super::prefix_chunks::from_value) 的先例。
//!
//! ## 形状是 `[[name, text], …]`，不是对象数组
//!
//! 两个字段都是位置固定的字符串，键名进落盘字节纯属浪费——跟
//! [`prefix_chunks`](super::prefix_chunks) 同一个形状选择（那边是 `[label, text]`）。

use std::sync::Arc;

use crate::value::atom_value::AgentValue;

/// 按 name 排序 → `[[name, text], …]`（红线 11，见模块文档「排序」）。
pub(crate) fn to_value(mut entries: Vec<(Arc<str>, Arc<str>)>) -> AgentValue {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let items: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|(name, text)| {
            serde_json::Value::Array(vec![
                serde_json::Value::String(name.to_string()),
                serde_json::Value::String(text.to_string()),
            ])
        })
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(items)))
}

/// 从值里读回声明（原样顺序，写入时已排序）。
///
/// 形状不对——整份读空，不逐项跳过（见模块文档「反序列化」）。类型不对（别的槽位
/// 误读到这里）或压根没写过：同样回到空，不 panic——这个读取点也服务恢复路径，
/// 一份认不出的历史数据不该让整个会话起不来。
pub(crate) fn from_value(value: &AgentValue) -> Vec<(Arc<str>, Arc<str>)> {
    let Some(array) = value.as_json().and_then(|j| j.as_array()) else {
        return Vec::new();
    };
    let mut entries = Vec::with_capacity(array.len());
    for item in array {
        // 切片模式钉死「恰好两项」：多一项少一项都是这份值坏了。
        let Some([name, text]) = item.as_array().map(Vec::as_slice) else {
            return Vec::new();
        };
        let (Some(name), Some(text)) = (name.as_str(), text.as_str()) else {
            return Vec::new();
        };
        entries.push((Arc::from(name), Arc::from(text)));
    }
    entries
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(name: &str, text: &str) -> (Arc<str>, Arc<str>) {
        (Arc::from(name), Arc::from(text))
    }

    /// 往返：写进去乱序，读回来按 name 排过序（红线 11）。
    #[test]
    fn round_trips_sorted_by_name() {
        let entries = vec![entry("zeta", "后声明的"), entry("alpha", "先声明的")];
        let read = from_value(&to_value(entries));

        assert_eq!(read.len(), 2);
        assert_eq!(&*read[0].0, "alpha", "写入时已按 name 排过序");
        assert_eq!(&*read[0].1, "先声明的");
        assert_eq!(&*read[1].0, "zeta");
        assert_eq!(&*read[1].1, "后声明的");
    }

    /// 空声明落成空数组（默认值就是它），读回来也是空——「没声明」和「声明了零个」
    /// 在状态上就是同一件事。
    #[test]
    fn no_declaration_is_an_empty_array_not_null() {
        let value = to_value(Vec::new());
        assert_eq!(value, crate::graph::Slot::HostPrefix.default_value());
        assert!(from_value(&value).is_empty());
    }

    /// 红线 11：两次序列化逐字节相同；顺序不同的输入排完序落成同一份字节。
    #[test]
    fn serialization_is_byte_stable_regardless_of_input_order() {
        let forward = vec![entry("b", "二"), entry("a", "一")];
        let backward = vec![entry("a", "一"), entry("b", "二")];

        let bytes = |v: &AgentValue| {
            let AgentValue::Json(json) = v else {
                panic!("落 Json")
            };
            serde_json::to_string(&**json).unwrap()
        };

        let a = to_value(forward);
        let b = to_value(backward.clone());
        assert_eq!(bytes(&a), bytes(&b), "排序之后跟输入顺序无关");
        assert_eq!(
            bytes(&a),
            bytes(&to_value(backward)),
            "同一份声明两次序列化也必须逐字节相同"
        );
        assert_eq!(bytes(&a), r#"[["a","一"],["b","二"]]"#);
    }

    /// 形状不对：整份读空，不是跳过坏的那一项拼出半份开局块（照
    /// `prefix_chunks` 的先例，跟 `host_tools` 刻意不同）。
    #[test]
    fn a_malformed_value_reads_as_empty_instead_of_a_partial_prefix() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());

        for bad in [
            json!([["ok", "正文"], "整项不是数组"]),
            json!([["ok", "正文"], ["少了正文"]]),
            json!([["ok", "正文"], ["name", 3]]),
            json!([["ok", "正文"], ["name", "text", "多一项"]]),
        ] {
            assert!(
                from_value(&AgentValue::Json(Arc::new(bad.clone()))).is_empty(),
                "{bad} 该整份读空，而不是留下前面那一块"
            );
        }
    }
}
