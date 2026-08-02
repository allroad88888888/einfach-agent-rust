//! 探针用的 HTTP 客户端。**任何路径上都不打印 key。**
//!
//! 与 `client.rs` 的分工：那个只做「发 JSON、解 usage」，这个还要拿原始 SSE 行、
//! 响应头、以及错误体 —— 因为分帧格式、头、错误形状本身就是被探的对象。

use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::time::Duration;

pub struct Probe {
    pub provider: String,
    pub model: String,
    pub url: String,
    pub key: String,
    pub out: Vec<Value>,
}

impl Probe {
    pub fn new(provider: String, model: String, url: String, key: String) -> Self {
        Probe { provider, model, url, key, out: Vec::new() }
    }
}

impl Probe {
    /// 非流式调用，返回 (HTTP 状态, 响应体)。错误体也要拿到 —— 错误形状本身是被探的对象。
    pub fn post(&self, body: &Value) -> (u16, Value) {
        match ureq::post(&self.url)
            .set("Authorization", &format!("Bearer {}", self.key))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(120))
            .send_json(body.clone())
        {
            Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(code, r)) => (
                code,
                r.into_json()
                    .unwrap_or_else(|_| json!({ "_raw": "非 JSON 响应体" })),
            ),
            Err(e) => (0, json!({ "_transport": e.to_string() })),
        }
    }

    /// 发一次请求，只取**响应头**。限流退避、配额可见性都指望它。
    pub fn headers(&self, body: &Value) -> Value {
        let take = |r: &ureq::Response| {
            let mut m = serde_json::Map::new();
            for name in r.headers_names() {
                let lower = name.to_lowercase();
                // 只收和限流/配额/请求追踪有关的，别把整包头倒进结果文件
                if lower.starts_with("x-ratelimit")
                    || lower.starts_with("ratelimit")
                    || lower == "retry-after"
                    || lower.starts_with("x-request")
                    || lower.starts_with("x-quota")
                {
                    m.insert(lower, json!(r.header(&name).unwrap_or_default()));
                }
            }
            Value::Object(m)
        };
        match ureq::post(&self.url)
            .set("Authorization", &format!("Bearer {}", self.key))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(60))
            .send_json(body.clone())
        {
            Ok(r) => json!({ "status": r.status(), "headers": take(&r) }),
            Err(ureq::Error::Status(c, r)) => json!({ "status": c, "headers": take(&r) }),
            Err(e) => json!({ "error": e.to_string() }),
        }
    }

    /// 流式调用，收原始 SSE 行。分帧格式本身是被探的对象，所以不做任何解析。
    pub fn stream(&self, body: &Value, max_lines: usize) -> Vec<String> {
        let mut body = body.clone();
        body["stream"] = json!(true);
        let resp = ureq::post(&self.url)
            .set("Authorization", &format!("Bearer {}", self.key))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(120))
            .send_json(body);
        let reader = match resp {
            Ok(r) => r.into_reader(),
            Err(ureq::Error::Status(c, r)) => {
                return vec![format!("HTTP {c}: {}", r.into_string().unwrap_or_default())];
            }
            Err(e) => return vec![format!("传输失败: {e}")],
        };
        BufReader::new(reader)
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .take(max_lines)
            .collect()
    }

    pub fn record(&mut self, name: &str, note: &str, data: Value) {
        println!("    {name:<26} {note}");
        self.out.push(json!({ "probe": name, "note": note, "data": data }));
    }
}
