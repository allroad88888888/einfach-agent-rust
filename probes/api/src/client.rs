//! HTTP 调用与单次观测。**任何路径上都不打印 key。**

use serde_json::{Value, json};
use std::time::Duration;

/// 一次调用的观测结果。
pub struct Obs {
    pub label: String,
    pub prompt_tokens: Option<u64>,
    pub hit: Option<(u64, &'static str)>,
    pub usage_raw: Value,
    pub error: Option<String>,
}

impl Obs {
    pub fn hit_tokens(&self) -> u64 {
        self.hit.map(|(n, _)| n).unwrap_or(0)
    }
    pub fn prompt(&self) -> u64 {
        self.prompt_tokens.unwrap_or(0)
    }
    pub fn to_json(&self) -> Value {
        json!({
            "label": self.label,
            "prompt_tokens": self.prompt_tokens,
            "hit_tokens": self.hit.map(|(n, _)| n),
            "hit_field": self.hit.map(|(_, f)| f),
            "usage": self.usage_raw,
            "error": self.error,
        })
    }
}

/// 一次探测运行的上下文。`nonce` 混进 system prompt 最前面，
/// 保证每次运行都是**冷缓存**开始 —— 否则上一轮跑出来的缓存会污染这一轮的判读。
pub struct Ctx {
    pub provider: String,
    pub model: String,
    pub url: String,
    pub key: String,
    pub delay: Duration,
    pub nonce: String,
    pub observations: Vec<Value>,
}

impl Ctx {
    pub fn pause(&self) {
        std::thread::sleep(self.delay);
    }

    /// 发一次请求并记录观测。调用方拿到 `Obs` 判读，同时观测已进 `observations`。
    pub fn call(&mut self, body: &Value, label: &str) -> Obs {
        let o = send(&self.url, &self.key, body, label);
        show(&o);
        self.observations.push(o.to_json());
        o
    }

    /// 发请求前先等一拍。缓存落盘是异步的，连发会读到未落盘的状态。
    pub fn call_after_pause(&mut self, body: &Value, label: &str) -> Obs {
        self.pause();
        self.call(body, label)
    }
}

fn send(url: &str, key: &str, body: &Value, label: &str) -> Obs {
    let resp = ureq::post(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(120))
        .send_json(body.clone());

    match resp {
        Ok(r) => match r.into_json::<Value>() {
            Ok(v) => {
                let usage = v.get("usage").cloned().unwrap_or(Value::Null);
                Obs {
                    label: label.to_string(),
                    prompt_tokens: prompt_tokens(&usage),
                    hit: cache_hit(&usage),
                    usage_raw: usage,
                    error: None,
                }
            }
            Err(e) => err(label, format!("响应不是 JSON: {e}")),
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            err(label, format!("HTTP {code}: {}", body.chars().take(240).collect::<String>()))
        }
        Err(e) => err(label, format!("{e}")),
    }
}

fn err(label: &str, msg: String) -> Obs {
    Obs {
        label: label.to_string(),
        prompt_tokens: None,
        hit: None,
        usage_raw: Value::Null,
        error: Some(msg),
    }
}

pub fn show(o: &Obs) {
    match &o.error {
        Some(e) => println!("    {:<30} 失败  {e}", o.label),
        None => {
            let hit = match o.hit {
                Some((n, _)) => format!("{n:>6}"),
                None => "  缺失".to_string(),
            };
            println!(
                "    {:<30} prompt={:<7} cached={hit}",
                o.label,
                o.prompt_tokens.map(|n| n.to_string()).unwrap_or("?".into())
            );
        }
    }
}

/// 只描述观测到的比例，不替人下结论。
pub fn verdict(baseline: u64, probed: u64) {
    if baseline == 0 {
        println!("      → 基准为 0，本组无法判读");
        return;
    }
    println!(
        "      → 相对基准保留 {:.1}%（{probed} / {baseline}）",
        probed as f64 / baseline as f64 * 100.0
    );
}

/// 从 usage 里挖缓存命中数。字段名各家不同，Kimi 未命中时整个字段缺失。
pub fn cache_hit(usage: &Value) -> Option<(u64, &'static str)> {
    const PATHS: &[(&str, &[&str])] = &[
        ("prompt_cache_hit_tokens", &["prompt_cache_hit_tokens"]),
        ("prompt_tokens_details.cached_tokens", &["prompt_tokens_details", "cached_tokens"]),
        ("cached_tokens", &["cached_tokens"]),
        ("cache_read_input_tokens", &["cache_read_input_tokens"]),
    ];
    for (label, path) in PATHS {
        let mut cur = usage;
        let mut ok = true;
        for seg in *path {
            match cur.get(seg) {
                Some(next) => cur = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && let Some(n) = cur.as_u64() {
            return Some((n, label));
        }
    }
    None
}

pub fn prompt_tokens(usage: &Value) -> Option<u64> {
    usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64)
}
