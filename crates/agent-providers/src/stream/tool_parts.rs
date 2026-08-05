//! wire 的 `tool_calls` 分片 → [`ContentBlock::ToolUse`]。
//!
//! 流式下一次工具调用是**分片**到达的：第一片带 `id` 和 `function.name`，之后
//! 每片只带一截 `function.arguments`，**按 `index` 累加不是覆盖**（三家一致，
//! probes/PROVIDERS.md §三）。非流式响应是同一个形状的一次性版本，所以两条
//! 路径共用这里——「wire 的一次工具调用长什么样」只该有一份实现。
//!
//! 用 `BTreeMap<u32, _>` 而不是 `HashMap`：块的顺序要跟 `index` 一致且确定
//! （红线 11）。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{ContentBlock, ToolCallId};
use serde_json::Value;

/// 一次工具调用的声明刚刚完整（`id` 和 `name` 都拿到了）。参数可能还在流。
pub(crate) struct Announced {
    pub index: u32,
    pub id: String,
    pub name: String,
}

#[derive(Default)]
struct Part {
    id: String,
    name: String,
    arguments: String,
    announced: bool,
}

#[derive(Default)]
pub(crate) struct ToolParts {
    parts: BTreeMap<u32, Part>,
}

impl ToolParts {
    /// 吃一个 `tool_calls[i]` 分片。返回 `Some` 表示这一片让某次调用的声明第一次
    /// 完整——上层据此吐 `ToolCallStarted`，同一次调用只吐一次。
    pub(crate) fn absorb(&mut self, frag: &Value) -> Option<Announced> {
        // `index` 缺失时归 0：三家实测都带 index，缺了说明只有一次调用。
        let index = frag.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
        let part = self.parts.entry(index).or_default();

        if let Some(id) = frag.get("id").and_then(Value::as_str) {
            part.id.push_str(id);
        }
        if let Some(f) = frag.get("function") {
            if let Some(name) = f.get("name").and_then(Value::as_str) {
                part.name.push_str(name);
            }
            if let Some(args) = f.get("arguments").and_then(Value::as_str) {
                part.arguments.push_str(args);
            }
        }

        if part.announced || part.id.is_empty() || part.name.is_empty() {
            return None;
        }
        part.announced = true;
        Some(Announced {
            index,
            id: part.id.clone(),
            name: part.name.clone(),
        })
    }

    /// 收尾成块。`name_from_wire` 把 wire 上的函数名还原成我们的工具全名——
    /// 谁做这个还原是各家的事，所以由调用方传进来（本模块不认识任何一家）。
    pub(crate) fn into_blocks(self, name_from_wire: fn(&str) -> Arc<str>) -> Vec<ContentBlock> {
        self.parts
            .into_values()
            .map(|p| ContentBlock::ToolUse {
                // id 为空也照原样出块：不自己铸 id——铸了就跟 `ToolResult` 配不上，
                // 而且这个洞会一路静默到工具结果对不上号。
                id: ToolCallId::new(p.id),
                name: name_from_wire(&p.name),
                input: Arc::new(parse_arguments(&p.arguments)),
            })
            .collect()
    }
}

/// `arguments` 字符串 → JSON。空串是「无参工具」的常见写法，给 `{}`；
/// **解析不了就原样留成字符串**，不丢原文也不假装成对象——工具层能据此报出
/// 一个说得清的错。
pub(crate) fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    /// `arguments` 分三片 → 拼出完整 JSON；声明只宣告一次。
    #[test]
    fn arguments_accumulate_across_fragments() {
        let mut parts = ToolParts::default();
        let a = parts.absorb(&json!({
            "index": 0, "id": "call_1", "type": "function",
            "function": {"name": "get_weather", "arguments": ""}
        }));
        let a = a.expect("拿到 id + name 就该宣告");
        assert_eq!(
            (a.index, a.id.as_str(), a.name.as_str()),
            (0, "call_1", "get_weather")
        );

        assert!(
            parts
                .absorb(&json!({"index": 0, "function": {"arguments": "{\"ci"}}))
                .is_none()
        );
        assert!(
            parts
                .absorb(&json!({"index": 0, "function": {"arguments": "ty\": \"北"}}))
                .is_none()
        );
        assert!(
            parts
                .absorb(&json!({"index": 0, "function": {"arguments": "京\"}"}}))
                .is_none()
        );

        let blocks = parts.into_blocks(identity);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, &ToolCallId::new("call_1"));
                assert_eq!(&**name, "get_weather");
                assert_eq!(**input, json!({"city": "北京"}));
            }
            other => panic!("期望 ToolUse，拿到 {other:?}"),
        }
    }

    /// 并行调用按 `index` 分开，块顺序跟 index 一致。
    #[test]
    fn parallel_calls_keep_index_order() {
        let mut parts = ToolParts::default();
        parts.absorb(
            &json!({"index": 1, "id": "b", "function": {"name": "get_time", "arguments": "{}"}}),
        );
        parts.absorb(
            &json!({"index": 0, "id": "a", "function": {"name": "get_weather", "arguments": "{}"}}),
        );
        let names: Vec<Arc<str>> = parts
            .into_blocks(identity)
            .iter()
            .map(|b| match b {
                ContentBlock::ToolUse { name, .. } => name.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            names,
            vec![Arc::from("get_weather"), Arc::<str>::from("get_time")]
        );
    }

    #[test]
    fn arguments_edge_cases() {
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments("   "), json!({}));
        assert_eq!(parse_arguments("{\"a\":1}"), json!({"a": 1}));
        assert_eq!(parse_arguments("{not json"), json!("{not json"));
    }
}
