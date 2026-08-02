//! core 的 `ToolSpec` 列表 → OpenAI 兼容的 `tools` 数组项，三家共用这个形状。
//!
//! **要不要把 `late_tools` 并进来是各家的判断，这里不做**：DeepSeek/GLM 没有
//! 消息级 tools，只能调这里的 `build(tools, late, max)` 把两者合并进顶层；
//! Kimi 有消息级通道，顶层调 `build(tools, &[], max)`，`late_tools` 另外走
//! `kimi::encode` 里单独拼的消息（PROVIDERS.md §二「中途加载」）。
//!
//! 超限从**尾部**裁：料单里的 `tools` 已经按优先级降序排好（ADAPTER.md §料
//! 单），`late` 排在 `tools` 之后，于是「开轮就在的高优先级工具」永远先于
//! 「中途加的」保留。

use agent_core::ToolSpec;
use serde_json::{Value, json};

use super::names;

pub struct Built {
    pub value: Value,
    pub kept: usize,
    pub dropped: usize,
}

pub fn build(tools: &[ToolSpec], late: &[ToolSpec], max: usize) -> Built {
    let all: Vec<&ToolSpec> = tools.iter().chain(late).collect();
    let kept = all.len().min(max);
    let value = Value::Array(all[..kept].iter().map(|t| one(t)).collect());
    Built { value, kept, dropped: all.len() - kept }
}

pub fn one(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": names::to_wire(&spec.name),
            "description": &*spec.description,
            "parameters": &*spec.schema,
        }
    })
}

/// 指定的工具在裁剪之后还在不在。`None` = 料单里压根没有这个工具（不是我们
/// 造成的，原样翻译，让对方去报错）；`Some(false)` = **我们裁掉的**，
/// 那就是一次必须上报的降级。
pub fn survives(tools: &[ToolSpec], late: &[ToolSpec], name: &str, max: usize) -> Option<bool> {
    tools
        .iter()
        .chain(late)
        .position(|t| &*t.name == name)
        .map(|pos| pos < max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: Arc::from(name),
            description: Arc::from("d"),
            schema: Arc::new(json!({"type": "object"})),
        }
    }

    #[test]
    fn wire_shape_is_openai_function() {
        let built = build(&[spec("srv:fs/read")], &[], 128);
        assert_eq!(
            built.value,
            json!([{
                "type": "function",
                "function": {
                    "name": "srv_3Afs_2Fread",
                    "description": "d",
                    "parameters": {"type": "object"}
                }
            }])
        );
        assert_eq!((built.kept, built.dropped), (1, 0));
    }

    /// 晚加的工具排在末尾；超过上限从尾部裁。
    #[test]
    fn late_tools_append_and_tail_is_truncated() {
        let base: Vec<ToolSpec> = (0..127).map(|i| spec(&format!("srv:t{i}"))).collect();
        let late = vec![spec("srv:late/a"), spec("srv:late/b")];
        let built = build(&base, &late, 128);
        assert_eq!((built.kept, built.dropped), (128, 1));

        let arr = built.value.as_array().unwrap();
        assert_eq!(arr[127]["function"]["name"], json!("srv_3Alate_2Fa"));
        assert_eq!(survives(&base, &late, "srv:late/a", 128), Some(true));
        assert_eq!(survives(&base, &late, "srv:late/b", 128), Some(false));
        assert_eq!(survives(&base, &late, "srv:nope", 128), None);
    }

    /// `max` 大到不会截断（Kimi「上限未公布」的用法）时，`dropped` 恒为 0。
    #[test]
    fn unbounded_max_never_drops() {
        let base: Vec<ToolSpec> = (0..500).map(|i| spec(&format!("srv:t{i}"))).collect();
        let built = build(&base, &[], usize::MAX);
        assert_eq!((built.kept, built.dropped), (500, 0));
    }
}
