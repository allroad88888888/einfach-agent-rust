//! `SendPlan ↔ AgentValue::Json` 的唯一一处编解码（100）。
//!
//! `AgentValue` 的变体集合在 026 定死（`atom_value.rs` 模块注释：「值 schema 一旦
//! 有日志落过盘就动不得了」），之后新增的槽位——`ToolsAllowed`、`SkillsActive`、
//! `HostTools`、`HostSkills`、`DisabledBuiltins`——全部复用既有变体，没有一个开了
//! 新的。`SendPlan` 延续这条先例：塞进 [`AgentValue::Json`]，跟 [`super::str_set`] /
//! [`super::host_tools`] 是同一层——「domain 值 ↔ `AgentValue`」的翻译，不碰 store、
//! 不判语义。
//!
//! 跟 `str_set` 不同的是这里不需要排序去重（那是「一组字符串」形状的规矩）：
//! `SendPlan` 自己已经是逐字段确定的值（099），这里只是照它自己的 `Serialize`
//! 转成 JSON，一步都不多加工。

use std::sync::Arc;

use crate::value::atom_value::AgentValue;
use crate::value::send_plan::SendPlan;

/// `SendPlan` → `AgentValue::Json`。
///
/// `expect` 而不是静默兜底：`SendPlan` 的字段只有 `Vec<ToolCallId>` / `usize` /
/// `Option<SummaryId>`，没有任何会让 `serde_json::to_value` 失败的形状（NaN 浮点、
/// 非字符串 map 键）——失败只可能是 `SendPlan` 的 derive 坏了，那是要当场炸出来的
/// bug，不是可以吞掉的运行时情况。
pub(crate) fn to_value(plan: &SendPlan) -> AgentValue {
    let json = serde_json::to_value(plan).expect("SendPlan 可序列化（红线 3）");
    AgentValue::Json(Arc::new(json))
}

/// 从值里读回 `SendPlan`。类型对不上或解析失败一律回退到 [`SendPlan::new()`]
/// （恒等元）——同 `str_set::from_value` 的「宁可空、不 panic」精神：这个读取点
/// 也服务恢复路径，一份形状不对的历史数据不该让整个会话起不来。
pub(crate) fn from_value(value: &AgentValue) -> SendPlan {
    value
        .as_json()
        .and_then(|json| serde_json::from_value(json.as_ref().clone()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{SummaryId, ToolCallId};

    /// 往返：非平凡的 `SendPlan`（三个字段都非零值）编解码后逐值相同。
    #[test]
    fn round_trips_a_non_pristine_plan() {
        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_1")]);
        plan.advance_boundary(2, Some(SummaryId::new("s1")))
            .unwrap();

        assert_eq!(from_value(&to_value(&plan)), plan);
    }

    /// pristine 值也走同一条编解码路径——不是特殊分支。
    #[test]
    fn round_trips_the_pristine_plan() {
        assert_eq!(from_value(&to_value(&SendPlan::new())), SendPlan::new());
    }

    /// 类型不对（别的槽位误读到这里）或压根没写过：回退到恒等元，不 panic。
    #[test]
    fn a_non_json_value_reads_as_pristine() {
        assert_eq!(from_value(&AgentValue::Null), SendPlan::new());
        assert_eq!(from_value(&AgentValue::U64(3)), SendPlan::new());
    }
}
