//! 料单 → Kimi 的请求体。三处这家独有的判断（其余机制全在 `crate::wire`）：
//!
//! 1. `late_tools` 走消息级通道，不并顶层——这是 Kimi 独有的零缓存代价路径
//!    （`late_tools_message`，PROVIDERS.md §二）。
//! 2. `MustUse(name)` 永远降级成 `required`——思考常开、指定函数在这家永久
//!    400（`translate_intent`）。
//! 3. temperature 钳到 1——非 1 直接 400（`clamp_temperature`）。
//!
//! 红线 11：同一份料两次组装逐字节相同（机制由 `wire::canonical` 保证）。

use std::sync::Arc;

use agent_core::{Adjustment, RequestIntent, ToolSpec};
use serde_json::{Map, Value, json};

use super::CACHE_BLOCK;
use crate::wire::{canonical, messages, numeric, prefix, tools};
use crate::{Encoded, Ingredients};

/// 上限「未公布」（PROVIDERS.md）：不编一个没测过的数字，两条通道都不截断。
const MAX_TOOLS: usize = usize::MAX;

pub(crate) fn encode(ing: &Ingredients<'_>) -> Encoded {
    let mut adjustments = Vec::new();

    // 顶层只放开轮就在的工具；late_tools 走消息级，见 `late_tools_message`。
    let built = tools::build(ing.tools, &[], MAX_TOOLS);

    let tool_choice = translate_intent(ing, &mut adjustments);
    let temperature = clamp_temperature(ing.config.temperature, &mut adjustments);

    let system = messages::system_text(ing.system);
    let mut history = messages::history(ing.messages);
    // 中途激活的 skill 正文走消息级：一条独立的 `role:system` 消息追加到 history
    // 末尾（038：~100% 保前缀、免费）。放在 late_tools 消息之前，顺序固定即可
    // （红线 11）。不报 Adjustment——这不是妥协，就是把 skill 正文挂在末尾。
    if let Some(msg) = messages::late_system_message(ing.late_system) {
        history.push(msg);
    }
    if !ing.late_tools.is_empty() {
        // 零缓存代价的通道：追加一条 role:system + tools（无 content）的消息，
        // 放在 history 的末尾——对仅扩展匹配的前缀比对而言这仍是一次严格延长。
        // 不报 LateToolsForcedIntoPrefix：这不是妥协，模型这轮就是看得见工具。
        history.push(late_tools_message(ing.late_tools));
    }

    let seg = prefix::SegmentBytes {
        tools: canonical(&built.value),
        system: canonical(&system.as_ref().map_or(Value::Null, |s| json!(s))),
        history: prefix::concat(&history),
    };
    let (drift, predicted_cache) = prefix::compare(&seg, ing.prev_prefix, CACHE_BLOCK, 0);

    Encoded {
        body: canonical(&body(ing, &built, tool_choice, temperature, system, history)),
        prefix: prefix::image(&seg),
        drift,
        predicted_cache,
        adjustments,
    }
}

/// `RequestIntent` → `tool_choice`。`MustUse(name)` 在这家**永久不可用**
/// （实测 400：`tool_choice 'specified' is incompatible with thinking
/// enabled`，思考常开、API 里没有关闭字段）——不看料单里这个工具存不存在，
/// 无条件降级，跟 DeepSeek「只有我们自己裁掉了才降级」的规则不一样。
fn translate_intent(ing: &Ingredients<'_>, adjustments: &mut Vec<Adjustment>) -> Option<Value> {
    match &ing.intent {
        RequestIntent::Free => None,
        RequestIntent::MustUseTool => Some(json!("required")),
        RequestIntent::MustUse(name) => {
            adjustments.push(Adjustment::ToolChoiceDowngraded {
                wanted: Arc::clone(name),
                used: Arc::from("required"),
            });
            Some(json!("required"))
        }
    }
}

/// 只接受 1：`None` 不传、不调整；`Some(1.0)` 原样传、不调整；其余钳成 1.0
/// 并报 `TemperatureOverridden`。
fn clamp_temperature(wanted: Option<f32>, adjustments: &mut Vec<Adjustment>) -> Option<Value> {
    let wanted = wanted?;
    if wanted != 1.0 {
        adjustments.push(Adjustment::TemperatureOverridden { wanted, used: 1.0 });
    }
    numeric::finite(1.0)
}

/// 中途加载工具的 wire 形状：`role: "system"` + `tools`，**没有 `content`
/// 字段**（PROVIDERS.md §二实测的形状，不是省略成 `null`，是键都不写）。
fn late_tools_message(late: &[ToolSpec]) -> Value {
    let value = tools::build(late, &[], MAX_TOOLS).value;
    json!({"role": "system", "tools": value})
}

fn body(
    ing: &Ingredients<'_>,
    built: &tools::Built,
    tool_choice: Option<Value>,
    temperature: Option<Value>,
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
    if let Some(t) = temperature {
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
    use crate::kimi::test_support::{ing, spec};
    use agent_core::{Segment, SystemChunk};

    /// 红线 11：同一份料两次 encode 逐字节相同。
    #[test]
    fn same_ingredients_encode_byte_identical() {
        let t = [spec("srv:fs/read"), spec("srv:fs/write")];
        let s = [SystemChunk { label: Arc::from("base"), text: Arc::from("你是助手") }];
        let mut a = ing();
        a.tools = &t;
        a.system = &s;
        assert_eq!(encode(&a).body, encode(&a).body);
    }

    /// `Free` 不带 tool_choice，也不产生任何 Adjustment。
    #[test]
    fn free_intent_sends_no_tool_choice() {
        let out = encode(&ing());
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert!(body.get("tool_choice").is_none());
        assert!(out.adjustments.is_empty());
    }

    /// `MustUseTool` 直接可用，无调整。
    #[test]
    fn must_use_tool_needs_no_adjustment() {
        let mut i = ing();
        i.intent = RequestIntent::MustUseTool;
        let out = encode(&i);
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["tool_choice"], json!("required"));
        assert!(out.adjustments.is_empty());
    }

    /// temperature：`None` 不传不调整；恰好 1.0 原样传不调整；其余钳成 1.0
    /// 并报 `TemperatureOverridden`。
    #[test]
    fn temperature_only_accepts_one() {
        let out = encode(&ing());
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert!(body.get("temperature").is_none());
        assert!(out.adjustments.is_empty());

        let mut cfg = crate::kimi::test_support::config().clone();
        cfg.temperature = Some(1.0);
        let mut i = ing();
        i.config = &cfg;
        let out = encode(&i);
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["temperature"], json!(1.0));
        assert!(out.adjustments.is_empty());

        cfg.temperature = Some(0.3);
        let mut i = ing();
        i.config = &cfg;
        let out = encode(&i);
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["temperature"], json!(1.0));
        assert_eq!(
            out.adjustments,
            vec![Adjustment::TemperatureOverridden { wanted: 0.3, used: 1.0 }]
        );
    }

    /// 中途加载的工具追加成消息级，顶层 `tools` 不变；不产生
    /// `LateToolsForcedIntoPrefix`——这不是妥协。
    #[test]
    fn late_tools_go_message_level_not_top_level() {
        let t = [spec("srv:fs/read")];
        let late = [spec("srv:late/a")];
        let mut i = ing();
        i.tools = &t;
        i.late_tools = &late;
        let out = encode(&i);
        assert!(out.adjustments.is_empty(), "{:?}", out.adjustments);

        let body: Value = serde_json::from_slice(&out.body).unwrap();
        let top_names: Vec<_> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(top_names, vec!["srv_3Afs_2Fread"]);

        let messages = body["messages"].as_array().unwrap();
        let tail = messages.last().unwrap();
        assert_eq!(tail["role"], json!("system"));
        assert!(tail.get("content").is_none(), "late tools 消息不该有 content 字段");
        assert_eq!(
            tail["tools"][0]["function"]["name"],
            json!("srv_3Alate_2Fa")
        );
    }

    /// 上一轮的 History 只是被延长（追加了 late tools 消息），前缀不算漂。
    #[test]
    fn late_tools_message_is_a_strict_extension_of_prior_history() {
        let mut first = ing();
        let cold = encode(&first);
        let mut prev = cold.prefix;
        prev.prompt_tokens = Some(5120);

        let late = [spec("srv:late/a")];
        first.late_tools = &late;
        first.prev_prefix = Some(&prev);
        let out = encode(&first);
        assert_eq!(out.drift, None, "追加消息级 tools 不该判漂");
        assert_eq!(out.predicted_cache, 5120 / 256 * 256);
    }

    /// `MustUse(name)` 无条件降级——即使工具就在料单里，也不像 DeepSeek 那样
    /// 尝试直接翻译成指定函数。
    #[test]
    fn must_use_named_tool_is_unconditionally_downgraded() {
        let t = [spec("srv:fs/read")];
        let mut i = ing();
        i.tools = &t;
        i.intent = RequestIntent::MustUse(Arc::from("srv:fs/read"));
        let out = encode(&i);
        assert_eq!(
            out.adjustments,
            vec![Adjustment::ToolChoiceDowngraded {
                wanted: Arc::from("srv:fs/read"),
                used: Arc::from("required"),
            }]
        );
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["tool_choice"], json!("required"));
    }

    /// 冷启动不算漂；换掉 system 后漂在 System 段。
    #[test]
    fn drift_points_at_the_changed_segment() {
        let s1 = [SystemChunk { label: Arc::from("base"), text: Arc::from("一") }];
        let s2 = [SystemChunk { label: Arc::from("base"), text: Arc::from("二") }];
        let mut first = ing();
        first.system = &s1;
        let cold = encode(&first);
        assert_eq!((cold.drift, cold.predicted_cache), (None, 0));

        let mut prev = cold.prefix;
        prev.prompt_tokens = Some(700);
        let mut second = ing();
        second.system = &s2;
        second.prev_prefix = Some(&prev);
        let out = encode(&second);
        assert_eq!((out.drift, out.predicted_cache), (Some(Segment::System), 0));
    }
}
