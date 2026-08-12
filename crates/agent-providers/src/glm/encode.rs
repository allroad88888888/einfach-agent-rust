//! 料单 → GLM 的请求体。骨架跟 DeepSeek 几乎一样（没有消息级 tools，晚加的
//! 只能并进顶层；按上限从尾部截断），两处不同：
//!
//! 1. `tool_choice` 指定函数不需要先关思考——GLM 的思考和 tool_choice 是两个
//!    独立轴，四种 `tool_choice` 取值开/关思考都能用（PROVIDERS.md §二）。
//! 2. M1 不发 `thinking` 字段（默认关，且发了就进缓存前缀，见 `mod.rs`）。
//!
//! 红线 11：同一份料两次组装逐字节相同（机制由 `wire::canonical` 保证）。

use std::sync::Arc;

use agent_core::{Adjustment, RequestIntent};
use serde_json::{Map, Value, json};

use super::{CACHE_BLOCK, LATE_TOOLS_COST_MULTIPLE, MAX_TOOLS, PREDICT_MIN};
use crate::wire::{canonical, messages, names, numeric, prefix, tools};
use crate::{Encoded, Ingredients};

/// GLM 拒绝 `image_url`，无图片支持。
pub(crate) fn encode(ing: &Ingredients<'_>) -> Encoded {
    let mut adjustments = Vec::new();

    let built = tools::build(ing.tools, ing.late_tools, MAX_TOOLS);
    if !ing.late_tools.is_empty() {
        // 没有消息级 tools，晚加的只能并进顶层——前缀作废，代价约 2x（真前缀树
        // 比 DeepSeek 的仅扩展匹配对「顶层变了」惩罚轻得多）。
        adjustments.push(Adjustment::LateToolsForcedIntoPrefix {
            count: u32::try_from(ing.late_tools.len()).unwrap_or(u32::MAX),
            est_cost_multiple: LATE_TOOLS_COST_MULTIPLE,
        });
    }
    if built.dropped > 0 {
        adjustments.push(Adjustment::ToolsTruncated {
            kept: u32::try_from(built.kept).unwrap_or(u32::MAX),
            dropped: u32::try_from(built.dropped).unwrap_or(u32::MAX),
        });
    }

    let tool_choice = translate_intent(ing, &mut adjustments);

    let system = messages::system_text(ing.system);
    let history = messages::history(ing.messages).messages;
    let seg = prefix::SegmentBytes {
        tools: canonical(&built.value),
        system: canonical(&system.as_ref().map_or(Value::Null, |s| json!(s))),
        history: prefix::concat(&history),
    };
    let (drift, predicted_cache) = prefix::compare(&seg, ing.prev_prefix, CACHE_BLOCK, PREDICT_MIN);

    Encoded {
        body: canonical(&body(ing, &built, tool_choice, system, history)),
        prefix: prefix::image(&seg),
        drift,
        predicted_cache,
        adjustments,
    }
}

/// `RequestIntent` → `tool_choice`。四种取值都直接支持，`MustUse(name)` 只有
/// 在**我们自己**把目标工具裁掉时才降级（跟 DeepSeek 同一条规则）——这里没有
/// 思考互斥的问题，所以不像 DeepSeek 那样额外强制关思考。
fn translate_intent(ing: &Ingredients<'_>, adjustments: &mut Vec<Adjustment>) -> Option<Value> {
    match &ing.intent {
        RequestIntent::Free => None,
        RequestIntent::MustUseTool => Some(json!("required")),
        RequestIntent::MustUse(name) => {
            if tools::survives(ing.tools, ing.late_tools, name, MAX_TOOLS) == Some(false) {
                adjustments.push(Adjustment::ToolChoiceDowngraded {
                    wanted: Arc::clone(name),
                    used: Arc::from("required"),
                });
                return Some(json!("required"));
            }
            Some(json!({"type": "function", "function": {"name": names::to_wire(name)}}))
        }
    }
}

fn body(
    ing: &Ingredients<'_>,
    built: &tools::Built,
    tool_choice: Option<Value>,
    system: Option<String>,
    history: Vec<Value>,
) -> Value {
    let mut msgs = Vec::with_capacity(history.len() + 1);
    if let Some(text) = system {
        msgs.push(json!({"role": "system", "content": text}));
    }
    msgs.extend(history);

    let mut body = Map::new();
    body.insert("model".into(), json!(&*ing.config.model));
    body.insert("messages".into(), Value::Array(msgs));
    if built.kept > 0 {
        body.insert("tools".into(), built.value.clone());
    }
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({"include_usage": true}));
    if let Some(tc) = tool_choice {
        body.insert("tool_choice".into(), tc);
    }
    // temperature 自由，原样透传，不产生 Adjustment。
    if let Some(t) = ing.config.temperature.and_then(numeric::finite) {
        body.insert("temperature".into(), t);
    }
    if let Some(m) = ing.config.max_tokens {
        body.insert("max_tokens".into(), json!(m));
    }
    // M1 故意不发 thinking 字段，见 `mod.rs` 的模块文档。
    Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glm::test_support::{ing, spec, tool_names};
    use agent_core::{RequestIntent, Segment, SystemChunk};

    /// 红线 11：同一份料两次 encode 逐字节相同。
    #[test]
    fn same_ingredients_encode_byte_identical() {
        let t = [spec("srv:fs/read"), spec("srv:fs/write")];
        let s = [SystemChunk {
            label: Arc::from("base"),
            text: Arc::from("你是助手"),
        }];
        let mut a = ing();
        a.tools = &t;
        a.system = &s;
        assert_eq!(encode(&a).body, encode(&a).body);
    }

    /// `Free` 不带 tool_choice，不产生任何 Adjustment。
    #[test]
    fn free_intent_sends_no_tool_choice() {
        let out = encode(&ing());
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert!(body.get("tool_choice").is_none());
        assert!(out.adjustments.is_empty());
    }

    /// `MustUse(name)` 直接翻译成指定函数，**无调整**——跟 Kimi 不同，GLM 真的
    /// 支持这个取值。
    #[test]
    fn must_use_named_tool_translates_directly_without_adjustment() {
        let t = [spec("srv:fs/read")];
        let mut i = ing();
        i.tools = &t;
        i.intent = RequestIntent::MustUse(Arc::from("srv:fs/read"));
        let out = encode(&i);
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(
            body["tool_choice"],
            json!({"type": "function", "function": {"name": "srv_3Afs_2Fread"}})
        );
        assert!(
            body.get("thinking").is_none(),
            "GLM 不需要为 tool_choice 碰 thinking"
        );
        assert!(out.adjustments.is_empty());
    }

    /// 晚加的工具并进顶层要报代价 2x；被我们裁掉的 `MustUse` 目标要报降级。
    #[test]
    fn late_tools_and_truncation_are_reported() {
        let base: Vec<_> = (0..128).map(|i| spec(&format!("srv:t{i}"))).collect();
        let late = [spec("srv:late/a")];
        let mut i = ing();
        i.tools = &base;
        i.late_tools = &late;
        i.intent = RequestIntent::MustUse(Arc::from("srv:late/a"));
        let out = encode(&i);

        assert_eq!(
            out.adjustments,
            vec![
                Adjustment::LateToolsForcedIntoPrefix {
                    count: 1,
                    est_cost_multiple: 2.0
                },
                Adjustment::ToolsTruncated {
                    kept: 128,
                    dropped: 1
                },
                Adjustment::ToolChoiceDowngraded {
                    wanted: Arc::from("srv:late/a"),
                    used: Arc::from("required"),
                },
            ]
        );
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["tool_choice"], json!("required"));
        assert_eq!(tool_names(&body).len(), 128);
    }

    /// 冷启动不算漂；换掉 system 后漂在 System 段。
    #[test]
    fn drift_points_at_the_changed_segment() {
        let s1 = [SystemChunk {
            label: Arc::from("base"),
            text: Arc::from("一"),
        }];
        let s2 = [SystemChunk {
            label: Arc::from("base"),
            text: Arc::from("二"),
        }];
        let mut first = ing();
        first.system = &s1;
        let cold = encode(&first);
        assert_eq!((cold.drift, cold.predicted_cache), (None, 0));

        let mut prev = cold.prefix;
        prev.prompt_tokens = Some(860);
        let mut second = ing();
        second.system = &s2;
        second.prev_prefix = Some(&prev);
        let out = encode(&second);
        assert_eq!((out.drift, out.predicted_cache), (Some(Segment::System), 0));
    }

    /// 块粒度 64：严格延长时预测按 64 向下取整（跟 DeepSeek 的 128、Kimi 的
    /// 256 不同，这是唯一在数字上体现出来的地方）。
    #[test]
    fn strict_extension_predicts_floor_by_block_64() {
        let mut first = ing();
        let cold = encode(&first);
        let mut prev = cold.prefix;
        prev.prompt_tokens = Some(3100);
        first.prev_prefix = Some(&prev);
        let out = encode(&first);
        assert_eq!(out.predicted_cache, 3100 / 64 * 64);
    }
}
