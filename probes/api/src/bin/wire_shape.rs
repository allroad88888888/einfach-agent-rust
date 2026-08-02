//! 三家 wire 行为的差异探针。
//!
//! 文档在关键处是空的：DeepSeek 的 tool_calls 页拿不到、GLM 只给了 SDK 用法没给
//! chunk 结构、Kimi 没说并行工具调用。这些恰恰是写 adapter 必须知道的，所以实测。
//!
//! 探针只记录观测，**不下结论** —— 结论人工判读后写进 probes/results/。

use probes_api::http::Probe;
use probes_api::{caps, config, fixture};
use serde_json::{Value, json};

const RESULT_PATH: &str = "../results/wire-shape.json";

/// 最小请求：够短就行，这些实验不测缓存。
fn base(p: &Probe, tools: Vec<Value>, ask: &str) -> Value {
    let mut b = json!({
        "model": p.model,
        "messages": [
            { "role": "system", "content": "你是测试助手，严格按要求调用工具。" },
            { "role": "user", "content": ask }
        ],
        "max_tokens": 256,
        "temperature": caps::temperature(&p.provider),
        "stream": false
    });
    if !tools.is_empty() {
        b["tools"] = Value::Array(tools);
    }
    b
}

fn weather_tool() -> Value {
    json!({"type":"function","function":{
        "name":"get_weather","description":"查一个城市的天气",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}})
}

fn time_tool() -> Value {
    json!({"type":"function","function":{
        "name":"get_time","description":"查一个城市的当前时间",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}})
}

/// S：流式分帧。三家的 chunk 形状、结束标记、usage 出现位置各不相同。
fn probe_stream(p: &mut Probe) {
    println!("\n  S 流式分帧");
    let b = base(p, vec![], "用一句话说明什么是缓存。");
    let lines = p.stream(&b, 6);
    p.record("stream.text.head", &format!("{} 行", lines.len()), json!(lines));

    let b = base(p, vec![weather_tool()], "北京天气怎么样？必须调用工具。");
    let lines = p.stream(&b, 12);
    p.record("stream.tool.head", &format!("{} 行", lines.len()), json!(lines));

    // 结束段：看 [DONE] 与 usage 在哪。
    let mut b2 = base(p, vec![], "只回答一个字：好");
    b2["stream_options"] = json!({ "include_usage": true });
    let lines = p.stream(&b2, 800);
    let tail: Vec<_> = lines.iter().rev().take(4).rev().cloned().collect();
    p.record("stream.tail", "含 stream_options.include_usage", json!(tail));
}

/// P：并行工具调用。一次响应里能不能出现多个 tool_calls。
fn probe_parallel(p: &mut Probe) {
    println!("\n  P 并行工具调用");
    let b = base(
        p,
        vec![weather_tool(), time_tool()],
        "同时告诉我北京的天气和上海的当前时间。必须调用工具，两个都要。",
    );
    let (code, body) = p.post(&b);
    let calls = body
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    p.record(
        "parallel.tool_calls",
        &format!("HTTP {code}，一次返回 {calls} 个调用"),
        body.pointer("/choices/0/message").cloned().unwrap_or(Value::Null),
    );
}

/// C：tool_choice 各取值，**并且分「默认思考」与「显式关思考」两组**。
///
/// 起因：DeepSeek v4-pro 在不传 thinking 时报 `Thinking mode does not support this
/// tool_choice` —— 说明它默认开着思考，而思考模式不支持 required / 指定函数。
/// Kimi 文档里也有同一条限制。所以这不是某家的怪癖，要两组对照才分得清
/// 「这家不支持这个取值」和「思考模式不支持这个取值」。
fn probe_tool_choice(p: &mut Probe) {
    println!("\n  C tool_choice 取值");
    for (label, thinking) in [("默认思考", None), ("显式关思考", Some(false))] {
        for choice in [
            json!("none"),
            json!("required"),
            json!({"type":"function","function":{"name":"get_weather"}}),
        ] {
            let mut b = base(p, vec![weather_tool(), time_tool()], "北京天气怎么样？");
            b["tool_choice"] = choice.clone();
            if let Some(on) = thinking
                && !caps::apply_thinking(&p.provider, &mut b, on)
            {
                continue; // 没有 thinking 字段的家只跑第一组
            }
            let (code, body) = p.post(&b);
            let n = body
                .pointer("/choices/0/message/tool_calls")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            p.record(
                &format!("tool_choice[{label}]={choice}"),
                &format!("HTTP {code}，{n} 个调用"),
                body.get("error").cloned().unwrap_or(json!("ok")),
            );
        }
    }
}

/// K：Kimi 文档称「指定函数与思考开启不兼容」，会 400。另两家有没有同样的限制？
fn probe_forced_tool_with_thinking(p: &mut Probe) {
    println!("\n  K 指定函数 + 开启思考");
    let mut b = base(p, vec![weather_tool()], "北京天气怎么样？");
    b["tool_choice"] = json!({"type":"function","function":{"name":"get_weather"}});
    if !caps::apply_thinking(&p.provider, &mut b, true) {
        println!("    跳过：无 thinking 字段");
        return;
    }
    let (code, body) = p.post(&b);
    p.record(
        "forced_tool+thinking",
        &format!("HTTP {code}"),
        body.get("error").cloned().unwrap_or(json!("成功")),
    );
}

/// H：响应头。限流退避要靠 Retry-After / X-RateLimit-*，三家都没文档化，只能看有没有。
fn probe_headers(p: &mut Probe) {
    println!("\n  H 响应头（限流/配额）");
    let h = p.headers(&base(p, vec![], "hi"));
    let n = h.pointer("/headers").and_then(Value::as_object).map(|m| m.len()).unwrap_or(0);
    p.record("headers.normal", &format!("{n} 个相关头"), h);
}

/// V：GLM 的服务端工具。retrieval / web_search / MCP 由 GLM 自己执行，
/// 我们的 router 不参与 —— 对应 Location::Provider。先摸清请求与结果的形状。
fn probe_server_side_tools(p: &mut Probe) {
    if p.provider != "glm" {
        return;
    }
    println!("\n  V 服务端工具（GLM web_search）");
    let mut b = base(p, vec![], "2026 年 7 月有什么重要的 AI 新闻？请用搜索。");
    b["tools"] = json!([{ "type": "web_search", "web_search": { "enable": true } }]);
    let (code, body) = p.post(&b);
    p.record(
        "server_tool.web_search",
        &format!("HTTP {code}"),
        json!({
            "error": body.get("error"),
            "message_keys": body.pointer("/choices/0/message")
                .and_then(Value::as_object).map(|m| m.keys().cloned().collect::<Vec<_>>()),
            "finish_reason": body.pointer("/choices/0/finish_reason"),
            "extra_top_level": body.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
        }),
    );
}

/// E：错误响应的形状。retry 逻辑要按它分类，形状不一致就得各写各的。
fn probe_errors(p: &mut Probe) {
    println!("\n  E 错误响应形状");
    let mut bad_model = base(p, vec![], "hi");
    bad_model["model"] = json!("definitely-not-a-real-model");
    let (code, body) = p.post(&bad_model);
    p.record("error.bad_model", &format!("HTTP {code}"), body);

    let mut bad_param = base(p, vec![], "hi");
    bad_param["max_tokens"] = json!(-1);
    let (code, body) = p.post(&bad_param);
    p.record("error.bad_param", &format!("HTTP {code}"), body);

    // 故意用错 key，看鉴权失败的形状。key 本身不会出现在输出里。
    let saved = std::mem::replace(&mut p.key, "sk-invalid-probe".into());
    let (code, body) = p.post(&base(p, vec![], "hi"));
    p.key = saved;
    p.record("error.bad_key", &format!("HTTP {code}"), body);
}

fn main() {
    let root = match config::load() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let only: Option<String> = std::env::args().nth(2).filter(|_| {
        std::env::args().nth(1).as_deref() == Some("--provider")
    });

    let mut report: Value = std::fs::read_to_string(RESULT_PATH)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));

    for (name, cfg) in &root.providers {
        if only.as_ref().is_some_and(|o| o != name) {
            continue;
        }
        println!("\n━━━ {name}  model={}  key={} ━━━", cfg.model, cfg.key_status());
        let Some(key) = cfg.resolve_key() else {
            println!("  跳过：未配置 key");
            continue;
        };
        let mut p = Probe::new(name.clone(), cfg.model.clone(), caps::endpoint(&cfg.base_url), key);
        probe_stream(&mut p);
        // 只跑流式那组时跳过其余 —— 重跑一次全套要十几个请求。
        if std::env::var("PROBE_STREAM_ONLY").is_ok() {
            report[name] = Value::Array(p.out);
            continue;
        }
        probe_parallel(&mut p);
        probe_tool_choice(&mut p);
        probe_forced_tool_with_thinking(&mut p);
        probe_errors(&mut p);
        probe_headers(&mut p);
        probe_server_side_tools(&mut p);
        report[name] = Value::Array(p.out);
    }

    if let Some(d) = std::path::Path::new(RESULT_PATH).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(RESULT_PATH, serde_json::to_string_pretty(&report).unwrap());
    println!("\n原始观测已写入 probes/results/wire-shape.json");
}

// fixture 目前只被 cache_prefix 用到，这里显式引用一下避免未使用告警。
#[allow(dead_code)]
fn _keep_fixture_linked() -> &'static str {
    fixture::ASK
}
