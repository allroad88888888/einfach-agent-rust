//! 「有序字符串集 ↔ [`AgentValue::Json`] 数组」的唯一一处编解码。
//!
//! 三处 primitive 状态是**同一个形状**：spawn 的工具子集（`Slot::ToolsAllowed`）、
//! 039 激活的 skill 集（`Slot::SkillsActive`）、076 关掉的内置工具名
//! （`Slot::DisabledBuiltins`）——都是「一组字符串，排序去重后落成 JSON 字符串
//! 数组」。抽成一处，是因为红线 11 的「排序去重才逐字节确定」这一步
//! 不能在三个地方各写一遍：写漏一处就是那一处**每轮全价**，而且不报错、功能正常，
//! 只在账单上浮出来（DeepSeek 上 120x）。
//!
//! 这个模块只做序列化，不碰 store、不判活名单语义（`Null` 在 `ToolsAllowed` 是
//! 「不活着」、在 `SkillsActive` 是「没这个槽」——那是调用点的解释，见各自的读口）。
//!
//! **去重也是这里的事**：`DisabledBuiltins` 是一份关闭名单，同一个名字写两遍跟写
//! 一遍是同一个意思，但落盘字节不一样——留着重复项就等于让「客户端手抖多写一行」
//! 变成一个不同的前缀家族。

use std::sync::Arc;

use crate::value::atom_value::AgentValue;

/// 排序去重 → JSON 字符串数组值（红线 11）。`serde_json::Value` 的数组保序、对象
/// 是 `BTreeMap`，所以同一组字符串两次序列化逐字节相同。
pub(crate) fn to_value(mut items: Vec<Arc<str>>) -> AgentValue {
    items.sort();
    items.dedup();
    let arr: Vec<serde_json::Value> = items
        .into_iter()
        .map(|s| serde_json::Value::String(s.to_string()))
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(arr)))
}

/// 从值里读回字符串列表（原样顺序，写入时已排序）。类型对不上一律空——读取点
/// 复用于恢复路径，一份历史数据形状不对不该整会话 panic（本仓最恨静默错值，但
/// 恢复路径上「宁可空、加一条 warn」比「panic 掉整个会话」更符合红线 3 的精神）。
pub(crate) fn from_value(value: &AgentValue) -> Vec<Arc<str>> {
    let Some(array) = value.as_json().and_then(|j| j.as_array()) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|v| v.as_str().map(Arc::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 红线 11 的最小实检：入参顺序不同、含重复，落值逐字节相同。
    #[test]
    fn sorted_and_deduped_regardless_of_input_order() {
        let a = to_value(vec![Arc::from("b"), Arc::from("a"), Arc::from("b")]);
        let b = to_value(vec![Arc::from("a"), Arc::from("b")]);
        assert_eq!(a, b);
        let AgentValue::Json(v) = &a else {
            panic!("落 Json")
        };
        assert_eq!(serde_json::to_string(&**v).unwrap(), r#"["a","b"]"#);
    }

    /// 往返：`to_value` 出来的值 `from_value` 读回原集合（排序后）。
    #[test]
    fn round_trips_through_the_value() {
        let items = vec![Arc::<str>::from("x"), Arc::from("y")];
        let read = from_value(&to_value(items));
        assert_eq!(read, vec![Arc::<str>::from("x"), Arc::from("y")]);
    }

    /// 非数组值（`Null` / 别的变体）读成空，不 panic。
    #[test]
    fn a_non_array_value_reads_as_empty() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());
    }
}
