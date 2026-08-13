//! 料单 → 最小内核的 OpenAI 请求体（175 决策二）。
//!
//! 这个文件最重要的部分是它**没做**的事。对照 `glm/encode.rs`：
//!
//! | | glm | 这里 |
//! |---|---|---|
//! | `temperature` | 原样透传 | **不发** |
//! | 工具按上限截断 | 128，超了报 `ToolsTruncated` | **不截**（上限未知） |
//! | 晚加工具的代价 | 报 `LateToolsForcedIntoPrefix{2.0x}` | 报，但**代价倍数未知** |
//! | `stream_options` | 发 `{include_usage:true}` | **不发** |
//!
//! 红线 11 照旧：同一份料两次组装逐字节相同（机制由 `wire::canonical` 保证）。

use agent_core::{Adjustment, RequestIntent};
use serde_json::{Map, Value, json};

use super::{CACHE_BLOCK, MAX_TOOLS, PREDICT_MIN};
use crate::wire::{canonical, messages, names, prefix, tools};
use crate::{Encoded, Ingredients};

pub(crate) fn encode(ing: &Ingredients<'_>) -> Encoded {
    let mut adjustments = Vec::new();

    // MAX_TOOLS 是 usize::MAX，所以 `built.dropped` 恒为 0——不报 `ToolsTruncated`。
    // 对面真有上限就让它报错，那是可见的失败（见 `mod.rs`）。
    let built = tools::build(ing.tools, ing.late_tools, MAX_TOOLS);
    if !ing.late_tools.is_empty() {
        // 有没有「消息级 tools」这条捷径**不可知**，只能按最保守的走：并进顶层。
        // 代价倍数同样不可知——填 1.0 表示「我们不知道，别拿这个数当估算」，
        // 而不是假装它便宜。这条 Adjustment 的价值在于**让人看见前缀被作废了**。
        adjustments.push(Adjustment::LateToolsForcedIntoPrefix {
            count: u32::try_from(ing.late_tools.len()).unwrap_or(u32::MAX),
            est_cost_multiple: 1.0,
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
    // PREDICT_MIN = u32::MAX ⇒ predicted_cache 恒 0（无预测）。drift 仍然算——
    // 它比对的是我们自己两轮之间的字节，跟对面是谁无关，是第 1 层兜底
    // （在花钱之前抓我们自己的序列化 bug），这一层对任何端点都成立。
    let (drift, predicted_cache) = prefix::compare(&seg, ing.prev_prefix, CACHE_BLOCK, PREDICT_MIN);

    Encoded {
        body: canonical(&body(ing, &built, tool_choice, system, history)),
        prefix: prefix::image(&seg),
        drift,
        predicted_cache,
        adjustments,
    }
}

/// `RequestIntent` → `tool_choice`。
///
/// **`tool_choice` 属不属于「最小内核」？** 属于——它是 OpenAI 规范里工具功能
/// 的一部分，一个连 `tool_choice` 都不认的端点也不会认 `tools`，那种情况下
/// 请求本来就跑不通。它跟 `temperature` 的区别是：`temperature` 是**采样偏好**
/// （不发就用对面的默认，语义无损），`tool_choice` 是**语义要求**（不发就等于
/// 悄悄把「必须调工具」降级成「随你」，那是静默妥协，红线级别的大忌）。
///
/// 这里不做 `survives` 检查——`MAX_TOOLS` 是 `usize::MAX`，我们从不截断，
/// 所以「目标工具被我们自己裁掉了」这种情况结构上不存在。
fn translate_intent(ing: &Ingredients<'_>, _adjustments: &mut [Adjustment]) -> Option<Value> {
    match &ing.intent {
        RequestIntent::Free => None,
        RequestIntent::MustUseTool => Some(json!("required")),
        RequestIntent::MustUse(name) => Some(json!({
            "type": "function",
            "function": { "name": names::to_wire(name) }
        })),
    }
}

/// **最小内核**。字段少是这个函数的全部意义，加字段之前先读 `mod.rs` 的契约。
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
    if let Some(tc) = tool_choice {
        body.insert("tool_choice".into(), tc);
    }
    if let Some(m) = ing.config.max_tokens {
        body.insert("max_tokens".into(), json!(m));
    }
    // 【故意不发】temperature / top_p / n / stream_options。
    // 契约与实测理由见 `mod.rs`；`ing.config.temperature` 在这里被丢弃且**不报
    // Adjustment**——那是契约边界不是运行时妥协，每轮报一条会把 Adjustment
    // 变成噪音，而它的全部价值在于稀有（决策 17：空的时候才叫原样执行了）。
    Value::Object(body)
}

#[cfg(test)]
#[path = "encode_tests.rs"]
mod tests;
