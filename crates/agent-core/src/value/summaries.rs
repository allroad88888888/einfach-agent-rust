//! 「摘要库 ↔ [`AgentValue::Json`] 数组」的唯一一处编解码（107）。
//!
//! `Slot::Summaries` 装的是这个 agent 历次压缩产出的**摘要正文本身**，形状是
//! `Vec<(SummaryId, Arc<str>)>`。三条形状约束各有理由，都不是随手定的：
//!
//! - **`Vec` 不是 `HashMap`/`HashSet`**（红线 11）：正文会被投影插进 prompt 的最
//!   前面（`send_plan::project`），容器的迭代顺序一漂，前缀缓存整段作废——功能
//!   完全正常，只在账单上浮出来（DeepSeek 上 120 倍）。
//! - **正文 `Arc<str>` 不是 `String`**（红线 5）：摘要是大值，而这个槽位的整份值
//!   每次压缩都要被读出来、追加一条、再写回去，克隆必须是指针拷贝。落进
//!   `AgentValue::Json` 之后，`PartialEq` 走的也是 `Arc::ptr_eq` 那条快路。
//! - **只增不删**：第 N 次压缩摘要的是「摘要 N−1 + 之后的消息」，但摘要 N−1
//!   仍要留在库里——回收了，`/undo` 之后再 `redo` 回到那一刻就取不回正文，投影
//!   会把边界作废（099「宁可多发，不可发空洞」），一整段历史无声地重新全价发出去。
//!
//! 跟 [`super::str_set`] 不同，这里**不排序去重**：键是 `SummaryId`，由边界值派生
//! （`command::apply_summary`），天然唯一、天然按边界递增追加。排序在这里是多余的
//! 一步，还会把「第 N 份摘要是第 N 次压缩产出的」这条时间序抹掉；确定性由写入点
//! 保证（同一份历史重放两次，追加顺序逐条相同）。
//!
//! 跟 [`super::send_plan_codec`] 一样，这里只做「domain 值 ↔ `AgentValue`」的翻译，
//! 不碰 store、不判语义：谁能进库、同一个边界能不能换一份正文，是命令层的事。

use std::sync::Arc;

use crate::ids::SummaryId;
use crate::value::atom_value::AgentValue;

/// 摘要库 → [`AgentValue::Json`]。
///
/// 每份摘要落成一个二元数组 `[id, 正文]`，整体是一个数组——`serde_json::Value`
/// 的数组保序，所以同一份库两次序列化逐字节相同（红线 11）。
///
/// `expect` 而不是静默兜底，同 `send_plan_codec::to_value` 的理由：入参只有
/// `Arc<str>`，没有任何会让 `serde_json::to_value` 失败的形状（NaN 浮点、非字符串
/// map 键），失败只可能是 derive 坏了，那是要当场炸出来的 bug。
pub(crate) fn to_value(summaries: &[(SummaryId, Arc<str>)]) -> AgentValue {
    let json = serde_json::to_value(summaries).expect("摘要库可序列化（红线 3）");
    AgentValue::Json(Arc::new(json))
}

/// 从值里读回摘要库（原样顺序）。类型对不上或解析失败一律回退到**空库**，
/// 同 `str_set::from_value` 的「宁可空、不 panic」：这个读取点也服务恢复路径，
/// 一份形状不对的历史数据不该让整个会话起不来。
///
/// 空库不是静默错值：摘要取不到时投影会**把边界作废**、把完整历史原样发出去
/// （099），代价是这一轮贵一点，不是缺一段。
pub(crate) fn from_value(value: &AgentValue) -> Vec<(SummaryId, Arc<str>)> {
    value
        .as_json()
        .and_then(|json| serde_json::from_value(json.as_ref().clone()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, text: &str) -> (SummaryId, Arc<str>) {
        (SummaryId::new(id), Arc::from(text))
    }

    /// 往返：多份摘要编解码后逐值相同，**顺序原样保留**。
    #[test]
    fn round_trips_and_keeps_the_insertion_order() {
        let library = vec![
            summary("summary@3", "前三条的摘要"),
            summary("summary@9", "前九条的摘要"),
        ];
        assert_eq!(from_value(&to_value(&library)), library);
    }

    /// 空库也走同一条路径——不是特殊分支（`Slot::Summaries` 的默认值就是它）。
    #[test]
    fn round_trips_the_empty_library() {
        assert_eq!(from_value(&to_value(&[])), Vec::new());
    }

    /// 红线 11 的最小实检：同一份库两次序列化逐字节相同，且顺序不同的两份**不等**
    /// ——这里没有集合语义，顺序就是内容的一部分。
    #[test]
    fn serialization_is_byte_stable_and_order_sensitive() {
        let ab = vec![summary("summary@1", "a"), summary("summary@2", "b")];
        let ba = vec![summary("summary@2", "b"), summary("summary@1", "a")];
        assert_eq!(to_value(&ab), to_value(&ab));
        assert_ne!(to_value(&ab), to_value(&ba));

        let AgentValue::Json(v) = &to_value(&ab) else {
            panic!("落 Json")
        };
        assert_eq!(
            serde_json::to_string(&**v).unwrap(),
            r#"[["summary@1","a"],["summary@2","b"]]"#
        );
    }

    /// 类型不对（别的槽位误读到这里）或压根没写过：回退到空库，不 panic。
    #[test]
    fn a_non_array_value_reads_as_an_empty_library() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());
    }
}
