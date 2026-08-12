//! 料单 → DeepSeek 的请求体。**唯一允许做模型相关判断的地方**（红线 12）。
//!
//! 序列化机制（messages/tools/前缀/规范序列化）全在 `crate::wire`，这里只做
//! 这家独有的判断：强制工具调用要求先关思考。
//!
//! 这里做的每一次妥协都推一条 `Adjustment`：静默妥协是本层头号大忌——功能正常，
//! 只在账单或「模型怎么没调那个工具」上浮出来（docs/ADAPTER.md）。
//!
//! 红线 11：同一份料两次组装必须**逐字节相同**（机制由 `wire::canonical` 保证）。

use std::sync::Arc;

use agent_core::{Adjustment, RequestIntent};
use serde_json::{Map, Value, json};

use super::{CACHE_BLOCK, LATE_TOOLS_COST_MULTIPLE, MAX_TOOLS};
use crate::wire::{canonical, messages, names, numeric, prefix, tools};
use crate::{Encoded, Ingredients};

/// DeepSeek 拒绝 `image_url`，无图片支持。
pub(crate) fn encode(ing: &Ingredients<'_>) -> Encoded {
    let mut adjustments = Vec::new();

    let built = tools::build(ing.tools, ing.late_tools, MAX_TOOLS);
    if !ing.late_tools.is_empty() {
        // 没有消息级 tools，晚加的只能并进顶层——整条前缀这轮作废。
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
    if tool_choice.is_some() {
        // 实测：思考模式下 `required` 与指定函数都直接 400
        //（`Thinking mode does not support this tool_choice`）。关思考是唯一能过的
        // 路，但它**改变了模型行为**，必须让人看见。
        adjustments.push(Adjustment::ThinkingDisabledForToolChoice);
    }

    let system = messages::system_text(ing.system);
    let history = messages::history_with_tool_reasoning(ing.messages).messages;
    let seg = prefix::SegmentBytes {
        tools: canonical(&built.value),
        system: canonical(&system.as_ref().map_or(Value::Null, |s| json!(s))),
        // 逐条拼接，不套数组——追加一条消息必须是字节级的延长，见 `prefix::concat`。
        history: prefix::concat(&history),
    };
    let (drift, predicted_cache) = prefix::compare(&seg, ing.prev_prefix, CACHE_BLOCK, 0);

    Encoded {
        body: canonical(&body(ing, &built, tool_choice, system, history)),
        prefix: prefix::image(&seg),
        drift,
        predicted_cache,
        adjustments,
    }
}

/// `RequestIntent` → `tool_choice`。返回 `None` 表示不带这个字段。
fn translate_intent(ing: &Ingredients<'_>, adjustments: &mut Vec<Adjustment>) -> Option<Value> {
    match &ing.intent {
        RequestIntent::Free => None,
        RequestIntent::MustUseTool => Some(json!("required")),
        RequestIntent::MustUse(name) => {
            // 只有「**我们自己**把它裁掉了」才降级——那是我们造成的妥协。
            // 料单里压根没有这个工具是 core 那边的事，原样翻译，让对方报错，
            // 别在这里替 core 猜。
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
    // 流式是常态路径；usage 只有开了 include_usage 才会随尾帧回来，
    // 而丢了 usage 缓存兜底整轮失明（issue 024）。
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({"include_usage": true}));
    if let Some(tc) = tool_choice {
        body.insert("tool_choice".into(), tc);
        body.insert("thinking".into(), json!({"type": "disabled"}));
    }
    // temperature 这家自由，原样透传，不产生 Adjustment（只有被改了才叫妥协）。
    if let Some(t) = ing.config.temperature.and_then(numeric::finite) {
        body.insert("temperature".into(), t);
    }
    if let Some(m) = ing.config.max_tokens {
        body.insert("max_tokens".into(), json!(m));
    }
    Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deepseek::test_support::{ing, spec, tool_names};
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

    /// `Free` 不带 tool_choice，也就不关思考。
    #[test]
    fn free_intent_sends_no_tool_choice() {
        let body: Value = serde_json::from_slice(&encode(&ing()).body).unwrap();
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("thinking").is_none());
        assert_eq!(body["stream"], json!(true));
        assert!(
            encode(&ing()).adjustments.is_empty(),
            "没妥协就不该有 Adjustment"
        );
    }

    /// `MustUseTool` / `MustUse(name)` 都必须同请求显式关思考，并报一条
    /// `ThinkingDisabledForToolChoice`——实测不关直接 400。
    #[test]
    fn forced_tool_choice_disables_thinking() {
        let t = [spec("srv:fs/read")];
        for (intent, expected) in [
            (RequestIntent::MustUseTool, json!("required")),
            (
                RequestIntent::MustUse(Arc::from("srv:fs/read")),
                json!({"type": "function", "function": {"name": "srv_3Afs_2Fread"}}),
            ),
        ] {
            let mut i = ing();
            i.tools = &t;
            i.intent = intent;
            let out = encode(&i);
            let body: Value = serde_json::from_slice(&out.body).unwrap();
            assert_eq!(body["tool_choice"], expected);
            assert_eq!(body["thinking"], json!({"type": "disabled"}));
            assert_eq!(
                out.adjustments,
                vec![Adjustment::ThinkingDisabledForToolChoice]
            );
        }
    }

    /// 晚加的工具并进顶层要报代价；被我们裁掉的 `MustUse` 目标要报降级。
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
                    est_cost_multiple: 120.0
                },
                Adjustment::ToolsTruncated {
                    kept: 128,
                    dropped: 1
                },
                Adjustment::ToolChoiceDowngraded {
                    wanted: Arc::from("srv:late/a"),
                    used: Arc::from("required"),
                },
                Adjustment::ThinkingDisabledForToolChoice,
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
        prev.prompt_tokens = Some(434);
        let mut second = ing();
        second.system = &s2;
        second.prev_prefix = Some(&prev);
        let out = encode(&second);
        assert_eq!((out.drift, out.predicted_cache), (Some(Segment::System), 0));
    }
}
