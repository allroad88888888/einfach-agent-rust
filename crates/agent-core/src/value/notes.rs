//! 「一个 agent 自己的草稿纸 ↔ [`AgentValue::Json`] 数组」的唯一一处编解码
//! （209，决策 35 §三）。
//!
//! `Slot::Notes` 装的就是这个形状：一张 `key → value` 的表。**存的时候是
//! `[[key, value], …]`，在内存里是 [`BTreeMap`]**——两边都有序，而有序在这里是
//! 红线 11 的硬要求：这张表会以 tool_result 的形式进 prompt，`HashMap` 的迭代
//! 顺序每次进程都不一样，功能完全正常，只是每一轮都全价（DeepSeek 上 120 倍）。
//!
//! ## 为什么容器是 map 而不是 `Vec`
//!
//! 跟隔壁 [`inbox`](super::inbox)、[`summaries`](super::summaries) 刻意不同：
//! 那两个是**流水账**（顺序 = 事情发生的先后，同一个 key 可以出现两次），这个是
//! **一张表**（同一个 key 写第二次是覆盖，不是追加）。用 `Vec` 就得在每个写入点
//! 自己做「找到就替换、找不到就追加」，写漏一处的症状是同一个 key 在表里出现
//! 两次、读回来看哪一份取决于遍历方向——不报错。
//!
//! ## 反序列化：all-or-empty
//!
//! 照 [`prefix_chunks`](super::prefix_chunks) / [`host_prefix`](super::host_prefix)
//! 的先例，不逐项跳过。这份值是**一次原子写入的一个整体**（写它的只有
//! [`to_value`] 一处），其中一项形状不对不是「有一项过时了」，是这份值坏了；
//! 跳掉那一项会拼出一张看着挺像样、少一行的草稿纸，而模型会照着那张表继续干活，
//! 没有任何一处会响。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::value::atom_value::AgentValue;

/// 一张草稿纸。`BTreeMap` 不是 `HashMap`——红线 11（见模块文档）。
pub type Notes = BTreeMap<Arc<str>, Arc<str>>;

/// 一张表 → `[[key, value], …]`。
///
/// **不需要显式排序**：`BTreeMap` 的迭代本来就是 key 升序，这正是选它的理由。
/// 换成 `HashMap` 的那一天这个函数一个字都不用改，而落盘字节会开始每轮不同——
/// 所以类型别名 [`Notes`] 是承重的，不是图省事。
pub(crate) fn to_value(notes: &Notes) -> AgentValue {
    let items: Vec<serde_json::Value> = notes
        .iter()
        .map(|(key, value)| {
            serde_json::Value::Array(vec![
                serde_json::Value::String(key.to_string()),
                serde_json::Value::String(value.to_string()),
            ])
        })
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(items)))
}

/// 从值里读回这张表。
///
/// 形状不对——整份读空，不逐项跳过（见模块文档）。类型不对（别的槽位误读到
/// 这里）或压根没写过：同样回到空，不 panic——这个读取点也服务恢复路径，
/// 一份认不出的历史数据不该让整个会话起不来。
pub(crate) fn from_value(value: &AgentValue) -> Notes {
    let Some(array) = value.as_json().and_then(|j| j.as_array()) else {
        return Notes::new();
    };
    let mut notes = Notes::new();
    for item in array {
        // 切片模式钉死「恰好两项」：多一项少一项都是这份值坏了。
        let Some([key, text]) = item.as_array().map(Vec::as_slice) else {
            return Notes::new();
        };
        let (Some(key), Some(text)) = (key.as_str(), text.as_str()) else {
            return Notes::new();
        };
        notes.insert(Arc::from(key), Arc::from(text));
    }
    notes
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn notes(pairs: &[(&str, &str)]) -> Notes {
        pairs
            .iter()
            .map(|(k, v)| (Arc::from(*k), Arc::from(*v)))
            .collect()
    }

    /// 往返：乱序塞进去，读回来是 key 升序（红线 11）。
    #[test]
    fn round_trips_in_key_order() {
        let read = from_value(&to_value(&notes(&[
            ("zeta", "最后写的"),
            ("alpha", "最先写的"),
            ("mid", "中间"),
        ])));
        let keys: Vec<&str> = read.keys().map(|k| &**k).collect();
        assert_eq!(keys, vec!["alpha", "mid", "zeta"]);
        assert_eq!(&*read["alpha"], "最先写的");
    }

    /// 空表落成空数组（默认值就是它）——「从没记过」和「记了又都删了」
    /// 在状态上就是同一件事，读取点不必区分。
    #[test]
    fn an_empty_pad_is_an_empty_array_not_null() {
        let value = to_value(&Notes::new());
        assert_eq!(value, crate::graph::Slot::Notes.default_value());
        assert!(from_value(&value).is_empty());
    }

    /// 红线 11：同一张表两次序列化逐字节相同，且跟插入顺序无关。
    #[test]
    fn serialization_is_byte_stable_regardless_of_insertion_order() {
        let bytes = |v: &AgentValue| {
            let AgentValue::Json(json) = v else {
                panic!("落 Json")
            };
            serde_json::to_string(&**json).unwrap()
        };

        let forward = to_value(&notes(&[("a", "一"), ("b", "二")]));
        let backward = to_value(&notes(&[("b", "二"), ("a", "一")]));
        assert_eq!(bytes(&forward), bytes(&backward));
        assert_eq!(bytes(&forward), r#"[["a","一"],["b","二"]]"#);
    }

    /// 形状不对：整份读空，而不是留下前面那几行拼出一张少一行的草稿纸。
    #[test]
    fn a_malformed_value_reads_as_empty_instead_of_a_partial_pad() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());

        for bad in [
            json!([["ok", "正文"], "整项不是数组"]),
            json!([["ok", "正文"], ["少了正文"]]),
            json!([["ok", "正文"], ["key", 3]]),
            json!([["ok", "正文"], ["key", "text", "多一项"]]),
        ] {
            assert!(
                from_value(&AgentValue::Json(Arc::new(bad.clone()))).is_empty(),
                "{bad} 该整份读空"
            );
        }
    }
}
