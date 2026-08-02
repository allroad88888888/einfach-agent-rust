//! 各家 API 的字段差异表。**这里只放实测确认过的**，猜的不写。

use serde_json::{Value, json};

/// 探到的事实：**Kimi K3 只接受 temperature = 1**，传 0 直接 400
/// `invalid temperature: only 1 is allowed for this model`。
///
/// temperature 只影响采样、不进被缓存的 prompt，所以各家取值不同不影响可比性。
pub fn temperature(provider: &str) -> f64 {
    match provider {
        "kimi" => 1.0,
        _ => 0.0,
    }
}

/// 给请求加「思考档位」。字段位置各家不同，不支持的返回 false（不瞎猜字段名）。
pub fn apply_effort(provider: &str, body: &mut Value, level: &str) -> bool {
    match provider {
        "deepseek" => {
            body["thinking"] = json!({ "type": "enabled", "reasoning_effort": level });
            true
        }
        "glm" => {
            body["thinking"] = json!({ "type": "enabled" });
            body["reasoning_effort"] = json!(level);
            true
        }
        _ => false,
    }
}

/// 只开关 `thinking`，不带 effort —— 用于隔离验证「thinking.type 是否进前缀」。
pub fn apply_thinking(provider: &str, body: &mut Value, enabled: bool) -> bool {
    match provider {
        "deepseek" | "glm" => {
            body["thinking"] = json!({ "type": if enabled { "enabled" } else { "disabled" } });
            true
        }
        _ => false,
    }
}

/// 各家合法档位不同，取两个确实存在的值。
pub fn effort_levels(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "deepseek" => Some(("high", "max")),
        "glm" => Some(("low", "high")),
        _ => None,
    }
}

/// 只有 Kimi K3 有消息级 tools：`role: system` + `tools`，无 `content` 字段。
pub fn supports_message_level_tools(provider: &str) -> bool {
    provider == "kimi"
}

pub fn append_message_level_tools(body: &mut Value, tools: Vec<Value>) {
    if let Some(arr) = body["messages"].as_array_mut() {
        arr.push(json!({ "role": "system", "tools": tools }));
    }
}

pub fn endpoint(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}
