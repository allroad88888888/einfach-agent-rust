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

use agent_core::{Adjustment, RequestIntent};
use serde_json::{Map, Value, json};

use super::CACHE_BLOCK;
use super::late_tools;
use crate::wire::{canonical, messages, numeric, prefix, tools};
use crate::{Encoded, Ingredients};

/// 上限「未公布」（PROVIDERS.md）：不编一个没测过的数字，两条通道都不截断。
const MAX_TOOLS: usize = usize::MAX;

pub(super) fn encode(ing: &Ingredients<'_>) -> Encoded {
    let mut adjustments = Vec::new();

    // 顶层只放开轮就在的工具；late_tools 走消息级，见 `late_tools_message`。
    let built = tools::build(ing.tools, &[], MAX_TOOLS);

    let tool_choice = translate_intent(ing, &mut adjustments);
    let temperature = clamp_temperature(ing.config.temperature, &mut adjustments);

    let system = messages::system_text(ing.system);
    let mut history = messages::history(ing.messages).messages;
    if !ing.late_tools.is_empty() {
        // 零缓存代价的通道：追加一条 role:system + tools（无 content）的消息，
        // 放在 history 的末尾——对仅扩展匹配的前缀比对而言这仍是一次严格延长。
        // 不报 LateToolsForcedIntoPrefix：这不是妥协，模型这轮就是看得见工具。
        history.push(late_tools::message(ing.late_tools));
    }

    let seg = prefix::SegmentBytes {
        tools: canonical(&built.value),
        system: canonical(&system.as_ref().map_or(Value::Null, |s| json!(s))),
        history: prefix::concat(&history),
    };
    let (drift, block_prediction) = prefix::compare(&seg, ing.prev_prefix, CACHE_BLOCK, 0);
    let predicted_cache = block_prediction;

    Encoded {
        body: canonical(&body(
            ing,
            &built,
            tool_choice,
            temperature,
            system,
            history,
        )),
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
